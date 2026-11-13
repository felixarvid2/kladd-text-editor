//! macOS: filer från Finder (dubbelklick, *Öppna med*, släpp på Dock-ikonen).
//!
//! macOS skickar inte filerna som argument utan som Apple-händelsen `odoc`
//! (`kAEOpenDocuments`) till den körande processen. winit äger programmets delegat och
//! implementerar varken `application:openFiles:` eller `application:openURLs:`, så
//! händelsen hamnar hos NSApplications standardhanterare, som visar ett felmeddelande.
//! Här registreras en egen hanterare hos `NSAppleEventManager` som lägger sökvägarna i
//! samma kanal som filer från andra Kladd-processer (`single::Incoming`).
//!
//! Hanteraren måste installeras EFTER att NSApplication har startat (`finishLaunching`
//! installerar sina egna hanterare och skriver annars över våra). `KladdApp::new` körs av
//! eframe inifrån winits `applicationDidFinishLaunching`, alltså efter det, men före att
//! den första Apple-händelsen levereras: filer som startade programmet kommer med.

#[cfg(any(target_os = "macos", test))]
use std::path::PathBuf;

/// Gör om en sträng ur ett Apple-händelsedeskriptor till en sökväg. Finder skickar
/// fil-URL:er (`file:///Users/a/Mina%20filer/x.md`), äldre avsändare ibland vanliga sökvägar.
#[cfg(any(target_os = "macos", test))]
fn path_from_descriptor_string(s: &str) -> Option<PathBuf> {
    if let Some(rest) = s.strip_prefix("file://") {
        // `file://localhost/sökväg` är också giltigt.
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        // Filreferens-URL:er (`/.file/id=...`) går inte att läsa som sökväg.
        if !rest.starts_with('/') || rest.starts_with("/.file/") {
            return None;
        }
        return Some(PathBuf::from(crate::percent_decode(rest)));
    }
    if s.starts_with('/') {
        return Some(PathBuf::from(s));
    }
    None
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;
    use std::sync::mpsc::Sender;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, NSObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_foundation::NSString;

    use crate::single::Incoming;

    /// 'aevt' (kCoreEventClass) och 'odoc' (kAEOpenDocuments).
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    /// '----' (keyDirectObject): parametern som håller filerna.
    const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    struct Ivars {
        tx: Sender<Incoming>,
        ctx: egui::Context,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[name = "KladdOpenDocumentsHandler"]
        #[ivars = Ivars]
        struct OpenHandler;

        impl OpenHandler {
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open_documents(&self, event: &AnyObject, _reply: &AnyObject) {
                let files = paths_from_event(event);
                let iv = self.ivars();
                if iv.tx.send(Incoming::Open(files)).is_ok() {
                    iv.ctx.request_repaint();
                }
            }
        }
    );

    /// Sökvägarna i `keyDirectObject` (en lista, eller ett enskilt värde).
    fn paths_from_event(event: &AnyObject) -> Vec<PathBuf> {
        let mut out = Vec::new();
        // SAFETY: `event` är en NSAppleEventDescriptor; alla anropade metoder finns där och
        // har de typer som anges (AEKeyword = u32, NSInteger = isize).
        unsafe {
            let direct: Option<Retained<AnyObject>> = msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT];
            let Some(direct) = direct else { return out };
            let n: isize = msg_send![&*direct, numberOfItems];
            let mut push = |d: &AnyObject| {
                let s: Option<Retained<NSString>> = msg_send![d, stringValue];
                if let Some(p) = s.and_then(|s| super::path_from_descriptor_string(&s.to_string())) {
                    out.push(p);
                }
            };
            if n == 0 {
                push(&direct);
            } else {
                // Listor är 1-indexerade.
                for i in 1..=n {
                    let item: Option<Retained<AnyObject>> = msg_send![&*direct, descriptorAtIndex: i];
                    if let Some(item) = item {
                        push(&item);
                    }
                }
            }
        }
        out
    }

    /// Registrerar hanteraren för `odoc`. Måste anropas på huvudtråden, efter att
    /// NSApplication startat. Anrop utanför huvudtråden gör ingenting.
    pub fn install(tx: Sender<Incoming>, ctx: egui::Context) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(manager_class) = AnyClass::get(c"NSAppleEventManager") else { return };
        let handler: Retained<OpenHandler> = {
            let this = mtm.alloc::<OpenHandler>().set_ivars(Ivars { tx, ctx });
            // SAFETY: `init` är NSObjects designerade initierare.
            unsafe { msg_send![super(this), init] }
        };
        // SAFETY: `sharedAppleEventManager` returnerar en NSAppleEventManager;
        // `setEventHandler:andSelector:forEventClass:andEventID:` tar (id, SEL, u32, u32) och
        // hanterarens metod har signaturen (event, reply) som Apple kräver.
        unsafe {
            let manager: Retained<AnyObject> = msg_send![manager_class, sharedAppleEventManager];
            let _: () = msg_send![
                &*manager,
                setEventHandler: &*handler,
                andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                forEventClass: CORE_EVENT_CLASS,
                andEventID: OPEN_DOCUMENTS
            ];
        }
        // NSAppleEventManager behåller inte hanteraren: den ska leva så länge processen.
        std::mem::forget(handler);
    }
}

#[cfg(target_os = "macos")]
pub use imp::install;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(path_from_descriptor_string("file:///Users/a/Mina%20filer/%C3%A5.md"), Some(PathBuf::from("/Users/a/Mina filer/å.md")));
        assert_eq!(path_from_descriptor_string("file://localhost/tmp/x.txt"), Some(PathBuf::from("/tmp/x.txt")));
    }

    #[test]
    fn plain_paths_pass_through() {
        assert_eq!(path_from_descriptor_string("/tmp/a b.txt"), Some(PathBuf::from("/tmp/a b.txt")));
    }

    #[test]
    fn unusable_strings_are_skipped() {
        assert_eq!(path_from_descriptor_string("relativ.txt"), None);
        assert_eq!(path_from_descriptor_string("file:///.file/id=1234.5678"), None);
        assert_eq!(path_from_descriptor_string("https://example.com/x"), None);
        assert_eq!(path_from_descriptor_string(""), None);
    }
}
