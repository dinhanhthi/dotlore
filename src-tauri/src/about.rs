//! The application menu's About item.
//!
//! Tauri's default item sets the name and version only. With no icon, AppKit
//! draws the generic application glyph for a binary that is not a bundled
//! `.app`. On macOS the panel ignores comments, license, and website; the
//! lines under the version are `credits`, and the line at the bottom is
//! `copyright`. A plain credits string is left-aligned, so this item draws
//! the panel itself and centers that block.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, Message};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSAboutPanelOptionApplicationName,
    NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits, NSApplication, NSFont,
    NSFontAttributeName, NSImage, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSTextAlignment,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSData, NSDictionary, NSMutableAttributedString, NSRange, NSSize,
    NSString,
};
use tauri::menu::{MenuItem, MenuItemKind};
use tauri::App;

/// 128px so the panel draws a 128-point icon. AppKit does not scale it: the
/// image's pixel size becomes the point size, and the 256px tray logo would
/// fill the window.
const LOGO_PNG: &[u8] = include_bytes!("../icons/128x128.png");
const LOGO_POINTS: f64 = 128.0;

const ABOUT_ID: &str = "about";

const GITHUB: &str = "github.com/dinhanhthi/dotlore";
const AUTHOR: &str = "Made by Anh-Thi DINH";
const LICENSE: &str = "MIT License";

fn credits() -> String {
    format!("{GITHUB}\n{AUTHOR}")
}

/// Replace the default About item. The new item goes in first so a later
/// failure never leaves the menu without About.
pub fn install(app: &App) -> tauri::Result<()> {
    let Some(menu) = app.menu() else {
        eprintln!("dotlore: about: no app menu");
        return Ok(());
    };
    let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() else {
        eprintln!("dotlore: about: app menu is not a submenu");
        return Ok(());
    };
    let Some(MenuItemKind::Predefined(current)) = app_menu.items()?.into_iter().next() else {
        eprintln!("dotlore: about: first item is not the About item");
        return Ok(());
    };
    if !current.text()?.starts_with("About") {
        eprintln!("dotlore: about: first item is not the About item");
        return Ok(());
    }

    let name = app.package_info().name.clone();
    let about = MenuItem::with_id(
        app.handle(),
        ABOUT_ID,
        format!("About {name}"),
        true,
        None::<&str>,
    )?;
    app_menu.insert(&about, 0)?;
    app_menu.remove(&current)?;

    app.on_menu_event(|app, event| {
        if event.id().as_ref() == ABOUT_ID {
            show(app);
        }
    });
    Ok(())
}

fn show(app: &tauri::AppHandle) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("dotlore: about: not on the main thread");
        return;
    };
    let info = app.package_info();
    present(mtm, &info.name, &info.version.to_string());
}

fn present(mtm: MainThreadMarker, name: &str, version: &str) {
    let mut keys: Vec<&NSString> = Vec::new();
    let mut objects: Vec<Retained<AnyObject>> = Vec::new();

    keys.push(unsafe { NSAboutPanelOptionApplicationName });
    objects.push(as_any(NSString::from_str(name)));

    keys.push(unsafe { NSAboutPanelOptionApplicationVersion });
    objects.push(as_any(NSString::from_str(version)));

    keys.push(ns_string!("Copyright"));
    objects.push(as_any(NSString::from_str(LICENSE)));

    keys.push(unsafe { NSAboutPanelOptionCredits });
    objects.push(as_any(centered(&credits())));

    if let Some(icon) = logo() {
        keys.push(unsafe { NSAboutPanelOptionApplicationIcon });
        objects.push(as_any(icon));
    }

    let options = NSDictionary::from_retained_objects(&keys, &objects);
    unsafe {
        NSApplication::sharedApplication(mtm).orderFrontStandardAboutPanelWithOptions(&options);
    }
}

/// Credits sit in the panel as an attributed string. Without a centered
/// paragraph style, AppKit left-aligns the wrapped description. The small
/// system font keeps them below the name and version.
fn centered(text: &str) -> Retained<NSMutableAttributedString> {
    let attributed = NSMutableAttributedString::from_nsstring(&NSString::from_str(text));
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    let font = NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
    let range = NSRange::new(0, attributed.length());
    unsafe {
        attributed.addAttribute_value_range(NSParagraphStyleAttributeName, &style, range);
        attributed.addAttribute_value_range(NSFontAttributeName, &font, range);
    }
    attributed
}

fn logo() -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(LOGO_PNG);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(LOGO_POINTS, LOGO_POINTS));
    Some(image)
}

fn as_any(obj: Retained<impl Message>) -> Retained<AnyObject> {
    unsafe { Retained::cast_unchecked(obj) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_about_panel_names_the_product_its_repo_and_its_license() {
        let text = credits();
        assert_eq!(text, format!("{GITHUB}\n{AUTHOR}"));
        assert_eq!(AUTHOR, "Made by Anh-Thi DINH");
        assert!(!text.contains("Sync your AI stuff"), "{text}");
        assert_eq!(LICENSE, "MIT License");
    }
}
