//! The application menu's About item.
//!
//! Tauri's default item sets the name and version only. With no icon, AppKit
//! draws the generic application glyph for a binary that is not a bundled
//! `.app`. On macOS the panel ignores comments, license, and website; the
//! lines under the version are `credits`, and the line at the bottom is
//! `copyright`.

use tauri::image::Image;
use tauri::include_image;
use tauri::menu::{AboutMetadata, MenuItemKind, PredefinedMenuItem};
use tauri::App;

/// 128px so the panel draws a 128-point icon. AppKit does not scale it: the
/// image's pixel size becomes the point size, and the 256px tray logo would
/// fill the window.
const LOGO: Image<'_> = include_image!("icons/128x128.png");

const DESCRIPTION: &str = "Sync your AI stuff and keep it away from your main codebase.";
const GITHUB: &str = "github.com/dinhanhthi/dotlore";
const LICENSE: &str = "MIT License";

fn credits() -> String {
    format!("{DESCRIPTION}\n\n{GITHUB}")
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

    let info = app.package_info();
    let about = PredefinedMenuItem::about(
        app.handle(),
        None,
        Some(AboutMetadata {
            name: Some(info.name.clone()),
            version: Some(info.version.to_string()),
            copyright: Some(LICENSE.to_string()),
            credits: Some(credits()),
            icon: Some(LOGO.clone()),
            ..Default::default()
        }),
    )?;
    app_menu.insert(&about, 0)?;
    app_menu.remove(&current)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_about_panel_names_the_product_its_repo_and_its_license() {
        let text = credits();
        assert!(text.starts_with(DESCRIPTION), "{text}");
        assert!(text.ends_with(GITHUB), "{text}");
        assert_eq!(LICENSE, "MIT License");
    }
}
