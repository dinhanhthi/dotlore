//! Whether iCloud Drive has uploaded a file, from Foundation's URL resource
//! values. Asking in-process keeps a status poll from spawning a `brctl`
//! child per file.

use std::path::Path;

use objc2::rc::autoreleasepool;
use objc2_foundation::{
    NSArray, NSNumber, NSString, NSURLIsUbiquitousItemKey, NSURLUbiquitousItemIsUploadedKey, NSURL,
};

/// `Some(uploaded)` for an item in iCloud Drive, `None` for anything else —
/// a local file, a missing path, a failed query, and likely a Google Drive
/// file (unverified: a File Provider may report its own state). A
/// non-ubiquitous item has no upload state, so `None` keeps the caller from
/// reading "not uploaded" into a folder iCloud does not manage.
pub fn is_uploaded(path: &Path) -> Option<bool> {
    // Called from a Tokio blocking thread that has no pool of its own, so
    // without this the autoreleased URL and dictionary live as long as it.
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
        // SAFETY: both keys are Foundation's own immutable string constants.
        let (ubiquitous, uploaded) =
            unsafe { (NSURLIsUbiquitousItemKey, NSURLUbiquitousItemIsUploadedKey) };
        let values = url
            .resourceValuesForKeys_error(&NSArray::from_slice(&[ubiquitous, uploaded]))
            .ok()?;
        let flag = |key| {
            values
                .objectForKey(key)
                .and_then(|v| v.downcast::<NSNumber>().ok())
                .map(|n| n.boolValue())
        };
        if flag(ubiquitous) != Some(true) {
            return None;
        }
        flag(uploaded)
    })
}
