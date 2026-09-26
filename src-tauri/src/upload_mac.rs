//! Whether iCloud Drive has uploaded a file, from Foundation's URL resource
//! values. Asking in-process keeps a status poll from spawning a `brctl`
//! child per file.

use std::path::Path;

use objc2_foundation::{
    NSArray, NSNumber, NSString, NSURLIsUbiquitousItemKey, NSURLUbiquitousItemIsUploadedKey, NSURL,
};

/// `Some(uploaded)` for an item in iCloud Drive, `None` for anything else —
/// a local file, a Google Drive file, a missing path, or a failed query. A
/// non-ubiquitous item has no upload state, so `None` keeps the caller from
/// reading "not uploaded" into a folder iCloud does not manage.
pub fn is_uploaded(path: &Path) -> Option<bool> {
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
}
