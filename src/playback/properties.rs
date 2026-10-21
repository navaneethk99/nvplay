use super::ffi::Node;
use std::ffi::CStr;

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub language: String,
    pub codec: String,
    pub channels: i64,
    pub selected: bool,
}
impl Track {
    pub fn label(&self) -> String {
        let mut parts = vec![if self.title.is_empty() {
            format!("Track {}", self.id)
        } else {
            self.title.clone()
        }];
        if !self.language.is_empty() {
            parts.push(self.language.clone());
        }
        if !self.codec.is_empty() {
            parts.push(self.codec.to_uppercase());
        }
        if self.channels > 0 {
            parts.push(format!("{} ch", self.channels));
        }
        parts.join(" · ")
    }
}

#[derive(Clone, Debug)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
}

// These helpers only borrow a property event's owned node tree during poll().
// No pointers or borrowed strings survive the next mpv_wait_event call.
impl Node {
    pub(super) unsafe fn items(&self) -> &[Node] {
        if !matches!(self.format, 7 | 8) {
            return &[];
        }
        let pointer = unsafe { self.value.list };
        if pointer.is_null() {
            return &[];
        }
        let list = unsafe { &*pointer };
        if list.len <= 0 || list.values.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(list.values, list.len as usize) }
    }
    unsafe fn field(&self, name: &[u8]) -> Option<&Node> {
        if self.format != 8 {
            return None;
        }
        let list = unsafe { self.value.list.as_ref()? };
        if list.keys.is_null() {
            return None;
        }
        for (index, item) in unsafe { self.items() }.iter().enumerate() {
            let key = unsafe { *list.keys.add(index) };
            if !key.is_null() && unsafe { CStr::from_ptr(key) }.to_bytes() == name {
                return Some(item);
            }
        }
        None
    }
    unsafe fn text(&self) -> String {
        if self.format != 1 {
            return String::new();
        }
        let string = unsafe { self.value.string };
        if string.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(string) }
                .to_string_lossy()
                .into_owned()
        }
    }
    unsafe fn integer(&self) -> i64 {
        if self.format == 4 {
            unsafe { self.value.integer }
        } else {
            0
        }
    }
    unsafe fn boolean(&self) -> bool {
        self.format == 3 && unsafe { self.value.flag != 0 }
    }
}

pub(super) unsafe fn tracks(node: &Node) -> Vec<Track> {
    unsafe { node.items() }
        .iter()
        .map(|item| {
            // SAFETY: all fields belong to the still-live MPV_FORMAT_NODE event.
            unsafe {
                Track {
                    id: item.field(b"id").map_or(0, |n| n.integer()),
                    kind: item.field(b"type").map_or_else(String::new, |n| n.text()),
                    title: item.field(b"title").map_or_else(String::new, |n| n.text()),
                    language: item.field(b"lang").map_or_else(String::new, |n| n.text()),
                    codec: item.field(b"codec").map_or_else(String::new, |n| n.text()),
                    channels: item.field(b"audio-channels").map_or(0, |n| n.integer()),
                    selected: item.field(b"selected").is_some_and(|n| n.boolean()),
                }
            }
        })
        .collect()
}
pub(super) unsafe fn devices(node: &Node) -> Vec<AudioDevice> {
    unsafe { node.items() }
        .iter()
        .map(|item| unsafe {
            AudioDevice {
                name: item.field(b"name").map_or_else(String::new, |n| n.text()),
                description: item
                    .field(b"description")
                    .map_or_else(String::new, |n| n.text()),
            }
        })
        .collect()
}
