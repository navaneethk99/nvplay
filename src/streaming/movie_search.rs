//! The public movie catalog identifies titles; Cinejoy resolves media on play.
use super::{cinejoy::Resolver, find_tool};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    process::{Child, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Movie {
    pub id: u64,
    pub title: String,
    pub year: String,
    pub poster_path: Option<String>,
}
impl Movie {
    pub fn label(&self) -> String {
        if self.year.is_empty() {
            self.title.clone()
        } else {
            format!("{} ({})", self.title, self.year)
        }
    }
    pub fn poster_url(&self) -> Option<String> {
        self.poster_path
            .as_deref()
            .filter(|p| valid_poster_path(p))
            .map(|p| format!("https://image.tmdb.org/t/p/w154{p}"))
    }
    pub fn source(&self) -> String {
        format!("https://cinejoy.pk/watch/movie/{}", self.id)
    }
}
fn valid_poster_path(path: &str) -> bool {
    path.len() <= 180
        && path.starts_with('/')
        && (path.ends_with(".jpg") || path.ends_with(".png"))
        && path[1..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        && !path.contains("..")
}
fn parse_results(text: &str) -> Result<Vec<Movie>, String> {
    let values: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "Movie search returned invalid results")?;
    let values = values
        .as_array()
        .ok_or("Movie search returned invalid results")?;
    Ok(values
        .iter()
        .take(20)
        .filter_map(|v| {
            let id = v["id"].as_u64().filter(|id| *id > 0)?;
            let title = v["title"].as_str()?.trim();
            if title.is_empty() {
                return None;
            }
            let year = v["year"].as_str().unwrap_or("");
            Some(Movie {
                id,
                poster_path: v["poster_path"]
                    .as_str()
                    .filter(|p| valid_poster_path(p))
                    .map(str::to_owned),
                title: title.chars().take(240).collect(),
                year: if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) {
                    year.into()
                } else {
                    String::new()
                },
            })
        })
        .collect())
}

pub struct SearchUpdate {
    pub movies: Vec<Movie>,
    pub done: bool,
    pub checked: usize,
    pub total: usize,
}
// Positive checks are short-lived hints only; playback always resolves afresh.
static AVAILABLE: OnceLock<Mutex<HashMap<u64, Instant>>> = OnceLock::new();
fn cached(id: u64) -> bool {
    AVAILABLE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&id)
        .is_some_and(|at| at.elapsed() < Duration::from_secs(120))
}
fn remember(id: u64) {
    let mut cache = AVAILABLE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    cache.retain(|_, at| at.elapsed() < Duration::from_secs(120));
    if cache.len() >= 256
        && let Some(id) = cache.iter().min_by_key(|(_, at)| **at).map(|(id, _)| *id)
    {
        cache.remove(&id);
    }
    cache.insert(id, Instant::now());
}
struct Check {
    rank: usize,
    movie: Movie,
    resolver: Resolver,
}
pub struct Search {
    child: Option<Child>,
    started: Instant,
    pending: VecDeque<(usize, Movie)>,
    checking: Vec<Check>,
    verified: Vec<(usize, Movie)>,
    checked: usize,
    total: usize,
}
impl Search {
    pub fn start(query: &str) -> Result<Self, String> {
        let deno = find_tool("NVPLAY_DENO", "deno")
            .ok_or("Movie search requires Deno. Install Deno, then restart nvplay.")?;
        let mut child = super::tool_command(&deno)
            .args([
                "run",
                "--no-prompt",
                "--quiet",
                "--allow-net=api.themoviedb.org",
                "-",
                query,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start movie search: {e}"))?;
        if let Err(e) = child
            .stdin
            .take()
            .unwrap()
            .write_all(include_bytes!("movie_search.ts"))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Could not start movie search: {e}"));
        }
        Ok(Self {
            child: Some(child),
            started: Instant::now(),
            pending: VecDeque::new(),
            checking: Vec::new(),
            verified: Vec::new(),
            checked: 0,
            total: 0,
        })
    }
    fn record(&mut self, rank: usize, movie: Movie, available: bool) {
        self.checked += 1;
        if available {
            remember(movie.id);
            self.verified.push((rank, movie));
        }
    }
    fn snapshot(&mut self, done: bool) -> SearchUpdate {
        self.verified.sort_by_key(|(rank, _)| *rank);
        SearchUpdate {
            movies: self
                .verified
                .iter()
                .map(|(_, movie)| movie.clone())
                .collect(),
            done,
            checked: self.checked,
            total: self.total,
        }
    }
    pub fn poll(&mut self) -> Option<Result<SearchUpdate, String>> {
        let mut changed = false;
        if let Some(child) = &mut self.child {
            let status = match child.try_wait() {
                Ok(Some(status)) => status,
                Ok(None) if self.started.elapsed() < Duration::from_secs(15) => return None,
                Ok(None) => return Some(Err("Movie search timed out. Please try again.".into())),
                Err(e) => return Some(Err(format!("Movie search failed: {e}"))),
            };
            if !status.success() {
                return Some(Err(
                    "Movie search is unavailable. Check your connection and try again.".into(),
                ));
            }
            let mut text = String::new();
            if let Err(e) = child
                .stdout
                .take()
                .unwrap()
                .take(32768)
                .read_to_string(&mut text)
            {
                return Some(Err(e.to_string()));
            }
            let movies = match parse_results(&text) {
                Ok(movies) => movies,
                Err(e) => return Some(Err(e)),
            };
            self.child = None;
            self.total = movies.len();
            for (rank, movie) in movies.into_iter().enumerate() {
                if cached(movie.id) {
                    self.verified.push((rank, movie));
                    self.checked += 1;
                } else {
                    self.pending.push_back((rank, movie));
                }
            }
            changed = true;
        }
        let mut index = 0;
        while index < self.checking.len() {
            if let Some(result) = self.checking[index].resolver.poll() {
                let check = self.checking.remove(index);
                self.record(check.rank, check.movie, result.is_ok());
                // The probe's temporary playlist is removed here. Play resolves
                // again so it does not depend on an expired signed URL.
                changed = true;
            } else {
                index += 1;
            }
        }
        let expired = self.started.elapsed() >= Duration::from_secs(60);
        if expired {
            self.pending.clear();
            self.checking.clear();
        } else {
            while self.checking.len() < 3 {
                let Some((rank, movie)) = self.pending.pop_front() else {
                    break;
                };
                match Resolver::check(&movie.source()) {
                    Ok(resolver) => self.checking.push(Check {
                        rank,
                        movie,
                        resolver,
                    }),
                    Err(error) => return Some(Err(error)),
                }
            }
        }
        let done = self.pending.is_empty() && self.checking.is_empty();
        if changed || done {
            Some(Ok(self.snapshot(done)))
        } else {
            None
        }
    }
}
impl Drop for Search {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Dropping each Resolver cancels all outstanding availability checks.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn results_distinguish_remakes_and_only_construct_numeric_provider_urls() {
        let movies = parse_results(r#"[{"id":210577,"title":"Gone Girl","year":"2014"},{"id":1434964,"title":"Gone Girl","year":"2025"},{"id":0,"title":"invalid"},{"id":"https://example.com","title":"invalid"}]"#).unwrap();
        assert_eq!(movies.len(), 2);
        assert_eq!(movies[0].label(), "Gone Girl (2014)");
        assert_ne!(movies[0].label(), movies[1].label());
        assert_eq!(movies[0].source(), "https://cinejoy.pk/watch/movie/210577");
        assert!(parse_results("{}").is_err());
        assert!(parse_results("[]").unwrap().is_empty());
    }
    #[test]
    fn unverified_and_failed_catalog_matches_are_never_published() {
        let movies = parse_results(r#"[{"id":900000001,"title":"Available later","year":"2020"},{"id":900000002,"title":"Unavailable","year":"2021"},{"id":900000003,"title":"Available first","year":"2022"}]"#).unwrap();
        let mut search = Search {
            child: None,
            started: Instant::now(),
            pending: movies.iter().cloned().enumerate().collect(),
            checking: Vec::new(),
            verified: Vec::new(),
            checked: 0,
            total: 3,
        };
        assert!(
            search.snapshot(false).movies.is_empty(),
            "TMDB metadata must not appear before verification"
        );
        search.record(1, movies[1].clone(), false);
        assert!(
            search.snapshot(false).movies.is_empty(),
            "unavailable match leaked into search"
        );
        search.record(2, movies[2].clone(), true);
        assert_eq!(search.snapshot(false).movies, vec![movies[2].clone()]);
        search.record(0, movies[0].clone(), true);
        let update = search.snapshot(true);
        assert_eq!(update.movies, vec![movies[0].clone(), movies[2].clone()]);
        assert_eq!(update.checked, update.total);
        assert!(update.done);
    }
    #[test]
    #[ignore = "requires Deno and public movie catalog access"]
    fn real_movie_search_finds_cinejoy_movie() {
        let mut search = Search::start("Gone Girl").unwrap();
        let movies = loop {
            if let Some(result) = search.poll() {
                let update = result.unwrap();
                if update.movies.iter().any(|m| m.id == 210577) || update.done {
                    break update.movies;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(movies.iter().any(|m| m.id == 210577 && m.year == "2014"));
    }
}
