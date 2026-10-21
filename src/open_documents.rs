//! Finder sends an Apple open-documents event, not command-line arguments.
//! Receive it without replacing winit's NSApplication delegate.
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct Inbox {
    paths: Mutex<Vec<PathBuf>>,
    ui: Mutex<Option<egui::Context>>,
}
impl Inbox {
    #[cfg(any(target_os = "macos", test))]
    fn receive(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(paths);
        if let Some(ui) = self.ui.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            ui.request_repaint();
        }
    }
    fn take(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.paths.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

pub struct OpenDocuments {
    inbox: Arc<Inbox>,
    #[cfg(target_os = "macos")]
    _registration: macos::Registration,
}
impl OpenDocuments {
    /// Register before the event loop launches. Cold-start Finder events can
    /// arrive before eframe creates its window or supplies an egui Context.
    pub fn new() -> Self {
        let inbox = Arc::new(Inbox {
            paths: Mutex::new(Vec::new()),
            ui: Mutex::new(None),
        });
        Self {
            #[cfg(target_os = "macos")]
            _registration: macos::Registration::new(inbox.clone()),
            inbox,
        }
    }
    pub fn attach(&self, ui: &egui::Context) {
        *self.inbox.ui.lock().unwrap_or_else(|p| p.into_inner()) = Some(ui.clone());
        ui.request_repaint();
    }
    pub fn take(&self) -> Vec<PathBuf> {
        self.inbox.take()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
        runtime::ProtocolObject, sel,
    };
    use objc2_foundation::{
        NSAppleEventDescriptor, NSAppleEventManager, NSNotificationCenter, NSObject,
        NSObjectProtocol, NSString,
    };

    const CORE_EVENT: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
    const FILE_URL: u32 = u32::from_be_bytes(*b"furl");

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = Arc<Inbox>]
        struct DocumentHandler;

        unsafe impl NSObjectProtocol for DocumentHandler {}

        impl DocumentHandler {
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open(&self, event: &NSAppleEventDescriptor, _reply: Option<&NSAppleEventDescriptor>) {
                self.ivars().receive(event_paths(event));
            }
        }
    );

    fn event_paths(event: &NSAppleEventDescriptor) -> Vec<PathBuf> {
        let Some(list) = event.paramDescriptorForKeyword(DIRECT_OBJECT) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        for index in 1..=list.numberOfItems() {
            let Some(item) = list.descriptorAtIndex(index) else {
                continue;
            };
            let url = item.fileURLValue().or_else(|| {
                item.coerceToDescriptorType(FILE_URL)
                    .and_then(|item| item.fileURLValue())
            });
            if let Some(url) = url
                && url.isFileURL()
                && let Some(path) = url.path()
            {
                paths.push(PathBuf::from(path.to_string()));
            }
        }
        paths
    }

    pub(super) struct Registration {
        manager: Retained<NSAppleEventManager>,
        _handler: Retained<DocumentHandler>,
        observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }
    impl Registration {
        pub(super) fn new(inbox: Arc<Inbox>) -> Self {
            let main = MainThreadMarker::new()
                .expect("document events must be registered on the UI thread");
            let handler: Retained<DocumentHandler> =
                unsafe { msg_send![super(DocumentHandler::alloc(main).set_ivars(inbox)), init] };
            let manager = NSAppleEventManager::sharedAppleEventManager();
            install(&manager, &handler);
            // AppKit installs default document handlers during its own setup.
            // Re-register at will-finish-launching, before the initial Finder
            // event is dispatched, without replacing winit's delegate.
            let launch_manager = manager.clone();
            let launch_handler = handler.clone();
            let block = block2::RcBlock::new(
                move |_notification: std::ptr::NonNull<objc2_foundation::NSNotification>| {
                    install(&launch_manager, &launch_handler);
                },
            );
            // SAFETY: notification callbacks run on the posting (main) thread;
            // the notification center copies the block and retains its captures.
            let observer = unsafe {
                NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                    Some(&NSString::from_str(
                        "NSApplicationWillFinishLaunchingNotification",
                    )),
                    None,
                    None,
                    &block,
                )
            };
            Self {
                manager,
                _handler: handler,
                observer,
            }
        }
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            let observer: &ProtocolObject<dyn NSObjectProtocol> = &self.observer;
            unsafe {
                NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref());
            }
            self.manager
                .removeEventHandlerForEventClass_andEventID(CORE_EVENT, OPEN_DOCUMENTS);
        }
    }
    fn install(manager: &NSAppleEventManager, handler: &DocumentHandler) {
        // SAFETY: the selector has the two-descriptor signature required by
        // NSAppleEventManager. Registration retains the receiver and its inbox.
        unsafe {
            manager.setEventHandler_andSelector_forEventClass_andEventID(
                handler,
                sel!(handleOpenDocuments:withReplyEvent:),
                CORE_EVENT,
                OPEN_DOCUMENTS,
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use objc2_foundation::{NSString, NSURL};
        #[test]
        fn apple_open_event_decodes_direct_parameter_file_urls() {
            let event = NSAppleEventDescriptor::appleEventWithEventClass_eventID_targetDescriptor_returnID_transactionID(CORE_EVENT, OPEN_DOCUMENTS, None, -1, 0);
            assert!(event_paths(&event).is_empty());
            let list = NSAppleEventDescriptor::listDescriptor();
            let paths = ["/tmp/movie #1.mp4", "/tmp/日本語 movie.mp4"];
            for (index, path) in paths.iter().enumerate() {
                let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                list.insertDescriptor_atIndex(
                    &NSAppleEventDescriptor::descriptorWithFileURL(&url),
                    index as isize + 1,
                );
            }
            event.setParamDescriptor_forKeyword(&list, DIRECT_OBJECT);
            assert_eq!(event_paths(&event), paths.map(PathBuf::from));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_batches_are_delivered_once_with_paths_intact() {
        let inbox = Inbox {
            paths: Mutex::new(Vec::new()),
            ui: Mutex::new(None),
        };
        let paths = vec![
            PathBuf::from("/tmp/clip #1.mp4"),
            PathBuf::from("/tmp/日本語 movie.mkv"),
        ];
        inbox.receive(paths.clone());
        assert_eq!(inbox.take(), paths);
        assert!(inbox.take().is_empty());
        inbox.receive([PathBuf::from("/tmp/second.mp4")]);
        assert_eq!(inbox.take(), [PathBuf::from("/tmp/second.mp4")]);
    }
}
