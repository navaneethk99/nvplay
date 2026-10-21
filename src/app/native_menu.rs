//! AppKit owns the menu bar; actions are queued back to the egui update thread.
use super::PlayerApp;
use eframe::egui;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::Sel, sel,
};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Open,
    Network,
    SearchMovies,
    Pause,
    Stop,
    Previous,
    Next,
    Repeat,
    Fullscreen,
    Subtitle,
    Playlist,
    Info,
    Help,
    Quit,
    CheckUpdates,
    AutomaticUpdates,
    Command(Vec<String>),
}
struct Inbox {
    actions: Mutex<Vec<Action>>,
    ctx: egui::Context,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = (Action, Arc<Inbox>)]
    struct MenuHandler;
    unsafe impl NSObjectProtocol for MenuHandler {}
    impl MenuHandler {
        #[unsafe(method(invoke:))]
        fn invoke(&self, _sender: &NSMenuItem) {
            let (action, inbox) = self.ivars();
            inbox.actions.lock().unwrap_or_else(|p| p.into_inner()).push(action.clone());
            inbox.ctx.request_repaint();
        }
    }
);

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    title: String,
    action: Option<Action>,
    key: String,
    shift: bool,
    enabled: bool,
    checked: bool,
    children: Vec<Entry>,
}
impl Entry {
    fn item(title: impl Into<String>, action: Action) -> Self {
        Self {
            title: title.into(),
            action: Some(action),
            key: String::new(),
            shift: false,
            enabled: true,
            checked: false,
            children: Vec::new(),
        }
    }
    fn command(title: impl Into<String>, args: &[&str]) -> Self {
        Self::item(
            title,
            Action::Command(args.iter().map(|s| (*s).to_owned()).collect()),
        )
    }
    fn submenu(title: &str, children: Vec<Self>) -> Self {
        Self {
            title: title.into(),
            action: None,
            key: String::new(),
            shift: false,
            enabled: true,
            checked: false,
            children,
        }
    }
    fn key(mut self, key: &str) -> Self {
        self.key = key.into();
        self
    }
    fn shift_key(mut self, key: &str) -> Self {
        self.key = key.into();
        self.shift = true;
        self
    }
    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
    fn separator() -> Self {
        Self::submenu("", Vec::new())
    }
}
struct Group {
    menu: Retained<NSMenu>,
    entries: Vec<Entry>,
    // NSMenuItem targets are weak. Keep receivers alive as long as their items.
    handlers: Vec<Retained<MenuHandler>>,
}
pub(super) struct NativeMenu {
    _root: Retained<NSMenu>,
    groups: Vec<Group>,
    inbox: Arc<Inbox>,
}
fn submenu(root: &NSMenu, title: &str, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    item.setSubmenu(Some(&menu));
    root.addItem(&item);
    menu
}
fn system_item(
    menu: &NSMenu,
    title: &str,
    action: Sel,
    key: &str,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    // SAFETY: these are standard AppKit responder-chain actions with one sender.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    menu.addItem(&item);
    item
}
impl NativeMenu {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let mtm = MainThreadMarker::new().expect("menus must be created on the UI thread");
        let app = NSApplication::sharedApplication(mtm);
        let root = NSMenu::new(mtm);
        let app_menu = submenu(&root, "nvplay", mtm);
        system_item(
            &app_menu,
            "About nvplay",
            sel!(orderFrontStandardAboutPanel:),
            "",
            mtm,
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let services = submenu(&app_menu, "Services", mtm);
        app.setServicesMenu(Some(&services));
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        system_item(&app_menu, "Hide nvplay", sel!(hide:), "h", mtm);
        let hide_others = system_item(
            &app_menu,
            "Hide Others",
            sel!(hideOtherApplications:),
            "h",
            mtm,
        );
        hide_others.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        );
        system_item(&app_menu, "Show All", sel!(unhideAllApplications:), "", mtm);
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        // Quit uses the same queued close path as the window close button.
        app_menu.setAutoenablesItems(false);
        let mut groups = vec![Group {
            menu: app_menu,
            entries: Vec::new(),
            handlers: Vec::new(),
        }];
        for title in [
            "Media", "Playback", "Audio", "Video", "Subtitle", "View", "Help",
        ] {
            if title == "Help" {
                let window = submenu(&root, "Window", mtm);
                system_item(&window, "Minimize", sel!(performMiniaturize:), "m", mtm);
                system_item(&window, "Zoom", sel!(performZoom:), "", mtm);
                app.setWindowsMenu(Some(&window));
            }
            let menu = submenu(&root, title, mtm);
            menu.setAutoenablesItems(false);
            if title == "Help" {
                app.setHelpMenu(Some(&menu));
            }
            groups.push(Group {
                menu,
                entries: Vec::new(),
                handlers: Vec::new(),
            });
        }
        app.setMainMenu(Some(&root));
        let mut result = Self {
            _root: root,
            groups,
            inbox: Arc::new(Inbox {
                actions: Mutex::new(Vec::new()),
                ctx: ctx.clone(),
            }),
        };
        let quit = Entry::item("Quit nvplay", Action::Quit).key("q");
        let updates = Entry::item("Check for Updates…", Action::CheckUpdates);
        let group = &mut result.groups[0];
        populate(
            &group.menu,
            &[updates, Entry::separator(), quit],
            &result.inbox,
            &mut group.handlers,
            mtm,
        );
        result
    }
    fn take(&self) -> Vec<Action> {
        std::mem::take(&mut *self.inbox.actions.lock().unwrap_or_else(|p| p.into_inner()))
    }
    fn sync(&mut self, entries: Vec<Vec<Entry>>) {
        let mtm = MainThreadMarker::new().expect("menus must be updated on the UI thread");
        for (group, entries) in self.groups.iter_mut().skip(1).zip(entries) {
            if group.entries == entries {
                continue;
            }
            group.menu.removeAllItems();
            group.handlers.clear();
            populate(&group.menu, &entries, &self.inbox, &mut group.handlers, mtm);
            group.entries = entries;
        }
    }
}
fn populate(
    menu: &NSMenu,
    entries: &[Entry],
    inbox: &Arc<Inbox>,
    handlers: &mut Vec<Retained<MenuHandler>>,
    mtm: MainThreadMarker,
) {
    for entry in entries {
        if entry.title.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(&entry.title));
        item.setEnabled(entry.enabled);
        item.setState(isize::from(entry.checked));
        item.setKeyEquivalent(&NSString::from_str(&entry.key));
        item.setKeyEquivalentModifierMask(if entry.shift {
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift
        } else {
            NSEventModifierFlags::Command
        });
        if let Some(action) = &entry.action {
            // SAFETY: NSObject init and the target's declared one-sender selector.
            // The group retains each target until its menu items are removed.
            let handler: Retained<MenuHandler> = unsafe {
                msg_send![
                    super(MenuHandler::alloc(mtm).set_ivars((action.clone(), inbox.clone()))),
                    init
                ]
            };
            unsafe {
                item.setTarget(Some(&handler));
                item.setAction(Some(sel!(invoke:)));
            }
            handlers.push(handler);
        } else {
            let child = NSMenu::new(mtm);
            child.setAutoenablesItems(false);
            populate(&child, &entry.children, inbox, handlers, mtm);
            item.setSubmenu(Some(&child));
        }
        menu.addItem(&item);
    }
}

impl PlayerApp {
    pub(super) fn update_native_menu(&mut self) {
        let Some(menu) = &self.native_menu else {
            return;
        };
        for action in menu.take() {
            self.menu_action(action);
        }
        let entries = self.native_entries();
        if let Some(menu) = &mut self.native_menu {
            menu.sync(entries);
        }
    }
    fn menu_action(&mut self, action: Action) {
        match action {
            Action::CheckUpdates => self.updates.check(&self.ctx),
            Action::AutomaticUpdates => self.updates.toggle_automatic(),
            Action::Open => self.pick(false),
            Action::Network => self.show_url = true,
            Action::SearchMovies => self.open_movie_search(),
            Action::Pause => self.toggle_pause(),
            Action::Stop => self.stop(),
            Action::Previous => self.previous(),
            Action::Next => self.next(),
            Action::Repeat => self.repeat = !self.repeat,
            Action::Fullscreen => self.toggle_fullscreen(&self.ctx.clone()),
            Action::Subtitle => self.pick(true),
            Action::Playlist => self.show_playlist = !self.show_playlist,
            Action::Info => self.show_info = !self.show_info,
            Action::Help => self.show_help = true,
            Action::Quit => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Command(args) => {
                self.command(&args.iter().map(String::as_str).collect::<Vec<_>>())
            }
        }
    }
    fn native_entries(&self) -> Vec<Vec<Entry>> {
        use Action::*;
        let state = self.player.as_ref().map(|p| &p.state);
        let available = state.is_some();
        let tracks = |kind: &str, property: &str| {
            let tracks: Vec<_> = state
                .into_iter()
                .flat_map(|s| &s.tracks)
                .filter(|t| t.kind == kind)
                .collect();
            if tracks.is_empty() {
                return vec![Entry::item(format!("No {kind} tracks"), Help).enabled(false)];
            }
            let mut entries = vec![
                Entry::command("Disable", &["set", property, "no"])
                    .checked(!tracks.iter().any(|t| t.selected)),
            ];
            entries.extend(tracks.iter().map(|t| {
                Entry::command(t.label(), &["set", property, &t.id.to_string()]).checked(t.selected)
            }));
            entries
        };
        let devices = state
            .map(|s| {
                s.audio_devices
                    .iter()
                    .map(|d| {
                        Entry::command(&d.description, &["set", "audio-device", &d.name])
                            .checked(d.name == s.audio_device)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let devices = if devices.is_empty() {
            vec![Entry::item("No audio devices", Help).enabled(false)]
        } else {
            devices
        };
        vec![
            vec![
                Entry::item("Open File…", Open)
                    .key("o")
                    .enabled(available && !self.dialog_busy),
                Entry::item("Open Network Stream…", Network)
                    .key("n")
                    .enabled(available),
                Entry::item("Search Movies…", SearchMovies)
                    .shift_key("m")
                    .enabled(available),
            ],
            vec![
                Entry::item(
                    if state.is_some_and(|s| !s.paused && !s.idle) {
                        "Pause"
                    } else {
                        "Play"
                    },
                    Pause,
                )
                .enabled(available),
                Entry::item("Stop", Stop).enabled(self.loading || state.is_some_and(|s| !s.idle)),
                Entry::item("Previous", Previous).enabled(self.current.is_some_and(|i| i > 0)),
                Entry::item("Next", Next).enabled(
                    super::next_index(self.current, self.queue.len(), self.repeat).is_some(),
                ),
                Entry::separator(),
                Entry::submenu(
                    "Speed",
                    [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0]
                        .into_iter()
                        .map(|v| {
                            Entry::command(format!("{v:.2}×"), &["set", "speed", &v.to_string()])
                                .checked(state.is_some_and(|s| (s.speed - v).abs() < 0.01))
                                .enabled(available)
                        })
                        .collect(),
                ),
                Entry::item("Repeat playlist", Repeat).checked(self.repeat),
            ],
            vec![
                Entry::submenu("Audio Track", tracks("audio", "aid")),
                Entry::submenu("Audio Device", devices),
                Entry::command("Mute", &["cycle", "mute"])
                    .checked(state.is_some_and(|s| s.muted))
                    .enabled(available),
            ],
            vec![
                Entry::submenu("Video Track", tracks("video", "vid")),
                Entry::item(
                    if self.fullscreen {
                        "Exit Fullscreen"
                    } else {
                        "Enter Fullscreen"
                    },
                    Fullscreen,
                ),
            ],
            vec![
                Entry::item("Add Subtitle File…", Subtitle).enabled(available && !self.dialog_busy),
                Entry::submenu("Subtitle Track", tracks("sub", "sid")),
                Entry::command("Show subtitles", &["cycle", "sub-visibility"])
                    .checked(state.is_some_and(|s| s.subtitles_visible))
                    .enabled(available),
            ],
            vec![
                Entry::item("Playlist", Playlist)
                    .key("l")
                    .checked(self.show_playlist),
                Entry::item("Playback information", Info).checked(self.show_info),
            ],
            vec![
                Entry::item("Keyboard shortcuts", Help),
                Entry::separator(),
                Entry::item("Check for Updates…", CheckUpdates),
                Entry::item("Automatically Install Updates", AutomaticUpdates)
                    .checked(self.updates.automatic),
            ],
        ]
    }
}

#[cfg(test)]
impl PlayerApp {
    pub(super) fn activate_test_menu(&mut self, path: &[&str]) {
        let groups = self.native_entries();
        let mut entries: Vec<&Entry> = groups.iter().flatten().collect();
        let mut selected = None;
        for label in path {
            let item = entries
                .iter()
                .find(|item| item.title.starts_with(label))
                .expect("native menu entry");
            assert!(item.enabled, "native menu entry is disabled");
            selected = Some((*item).clone());
            entries = item.children.iter().collect();
        }
        self.menu_action(selected.unwrap().action.expect("native menu action"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_menu_actions_and_checks_follow_app_state() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        let entries = app.native_entries();
        assert!(!entries[0][0].enabled);
        assert!(!entries[0][1].enabled);
        assert_eq!(entries[0][2].key, "m");
        assert!(entries[0][2].shift);
        assert_eq!(entries[0][2].action, Some(Action::SearchMovies));
        assert!(!entries[1][0].enabled);
        app.activate_test_menu(&["Playlist"]);
        app.activate_test_menu(&["Repeat playlist"]);
        app.activate_test_menu(&["Playback information"]);
        let entries = app.native_entries();
        assert!(entries[1].last().unwrap().checked);
        assert!(entries[5][0].checked);
        assert!(entries[5][1].checked);
        app.activate_test_menu(&["Playlist"]);
        assert!(!app.show_playlist);
        app.activate_test_menu(&["Keyboard shortcuts"]);
        assert!(app.show_help);
    }

    #[test]
    fn macos_has_no_in_window_menu_in_either_window_mode() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = PlayerApp::from_parts(&ctx, None, None, None, vec![]);
        app.initial_resize = false;
        for fullscreen in [false, true] {
            app.fullscreen = fullscreen;
            let output = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
            let update = output.platform_output.accesskit_update.unwrap();
            assert!(
                !update
                    .nodes
                    .iter()
                    .any(|(_, node)| node.role() == egui::accesskit::Role::MenuBar)
            );
            let video = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Callback(callback) => Some(callback.rect),
                    _ => None,
                })
                .expect("video surface");
            let topbar_height = if fullscreen { 0.0 } else { 34.0 };
            assert_eq!(video.top(), ctx.content_rect().top() + topbar_height);
        }
    }
}
