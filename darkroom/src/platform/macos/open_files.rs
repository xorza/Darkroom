//! Documents opened from Finder, on macOS.
//!
//! Launch Services delivers them as `application:openURLs:` on the
//! `NSApplicationDelegate` and never in argv, so without this the association
//! `assets/macos/Info.plist` declares launches the editor empty.
//!
//! **Why the selector is bolted onto winit's delegate rather than our own.**
//! winit registers a `WinitApplicationDelegate` in `EventLoop::new` and reads
//! it back with an `is_kind_of` check that panics on anything else
//! (rust-windowing/winit#4458), so the slot cannot be taken over and a
//! forwarding proxy cannot stand in front of it. Adding the method to the
//! class of the delegate winit already installed leaves the object exactly
//! what winit expects while teaching it one more message. The class is taken
//! from the live delegate rather than looked up by name, so nothing here
//! depends on the private symbol `WinitApplicationDelegate`.

use std::cell::RefCell;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{ffi, sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};

/// What a delivered path is handed to — `install`'s argument, kept for the
/// callbacks to find.
type Sink = Box<dyn Fn(PathBuf)>;

// Thread-local rather than a `static` because `install` and every callback run
// on the main thread, which is also what lets the sink hold a closure that is
// not `Send`. A plain function pointer cannot carry one, and the IMP the
// runtime calls is exactly that.
thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Teach winit's application delegate to hand Finder-opened documents to
/// `deliver`.
///
/// Call between building the host and running it: `EventLoop::new` must
/// already have installed the delegate, and the first `openURLs:` of a
/// double-click launch arrives once the loop is running.
///
/// Every failure here is loud, and deliberately so. A degraded install would
/// leave the editor advertising the association `Info.plist` declares while
/// silently dropping every document Finder sends — the one outcome worse than
/// not starting. All three conditions are code changes rather than bad data,
/// caught the first time a developer launches after touching winit, and this
/// is a cold startup path.
pub(crate) fn install(deliver: impl Fn(PathBuf) + 'static) {
    let mtm = MainThreadMarker::new().expect("install runs on the event loop's thread");
    let delegate = NSApplication::sharedApplication(mtm)
        .delegate()
        .expect("winit registered its application delegate in EventLoop::new");

    // SAFETY: `Retained` keeps a live object, and every Objective-C object is
    // an `AnyObject` — the cast reads its class, nothing more.
    let object: &AnyObject = unsafe { &*Retained::as_ptr(&delegate).cast::<AnyObject>() };
    let class: &AnyClass = object.class();

    // SAFETY: the signature matches `v@:@@` — the encoding of
    // `- (void)application:(id)app openURLs:(NSArray<NSURL *> *)urls`, which is
    // what the runtime will call this IMP with.
    let added = unsafe {
        ffi::class_addMethod(
            (class as *const AnyClass).cast_mut(),
            sel!(application:openURLs:),
            std::mem::transmute::<OpenUrls, unsafe extern "C-unwind" fn()>(open_urls),
            c"v@:@@".as_ptr(),
        )
    };
    // `class_addMethod` refuses rather than replaces, so false means the class
    // already answers the selector: winit implemented it upstream, or this ran
    // twice. Either way our IMP is not the one being called.
    assert!(
        added.as_bool(),
        "{:?} already implements application:openURLs: — winit may now expose it directly",
        class.name()
    );
    SINK.with_borrow_mut(|sink| *sink = Some(Box::new(deliver)));
}

type OpenUrls = extern "C" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>);

/// The added method. Runs on the main thread, called by AppKit.
extern "C" fn open_urls(_: &AnyObject, _: Sel, _: &AnyObject, urls: &NSArray<NSURL>) {
    let Some(path) = first_path(urls) else {
        return;
    };
    SINK.with_borrow(|sink| {
        if let Some(deliver) = sink {
            deliver(path);
        }
    });
}

/// The document to open out of everything Finder handed over.
///
/// Selecting several files and pressing Open delivers them in one call, but
/// the editor holds one document at a time — the same rule the CLI enforces by
/// rejecting a second path — so the rest are reported and dropped.
fn first_path(urls: &NSArray<NSURL>) -> Option<PathBuf> {
    if urls.len() > 1 {
        tracing::info!("opening the first of {} documents", urls.len());
    }
    let url = urls.iter().next()?;
    let Some(path) = url.path() else {
        tracing::warn!("opened URL is not a file path");
        return None;
    };
    Some(PathBuf::from(path.to_string()))
}

#[cfg(test)]
mod tests {
    use objc2_foundation::NSString;

    use super::*;

    fn urls(paths: &[&str]) -> Retained<NSArray<NSURL>> {
        let urls: Vec<_> = paths
            .iter()
            .map(|p| NSURL::fileURLWithPath(&NSString::from_str(p)))
            .collect();
        NSArray::from_retained_slice(&urls)
    }

    /// The multi-file policy, against arrays AppKit would really hand over:
    /// the first survives whatever follows it, and an empty drop is not a
    /// document to open.
    #[test]
    fn only_the_first_document_of_a_multi_file_open_is_taken() {
        assert_eq!(
            first_path(&urls(&[])),
            None,
            "nothing selected, nothing to open"
        );
        assert_eq!(
            first_path(&urls(&["/tmp/only.darkroom"])),
            Some(PathBuf::from("/tmp/only.darkroom"))
        );
        assert_eq!(
            first_path(&urls(&[
                "/tmp/first.darkroom",
                "/tmp/second.darkroom",
                "/tmp/third.darkroom",
            ])),
            Some(PathBuf::from("/tmp/first.darkroom")),
            "the trailing two are dropped, not merged or queued"
        );
    }

    /// `NSURL` percent-encodes on the way in, so the path handed to the editor
    /// has to come back out decoded — a space stays a space, not `%20`.
    #[test]
    fn a_file_url_decodes_back_to_its_filesystem_path() {
        assert_eq!(
            first_path(&urls(&["/tmp/a b/two words.darkroom"])),
            Some(PathBuf::from("/tmp/a b/two words.darkroom"))
        );
    }
}
