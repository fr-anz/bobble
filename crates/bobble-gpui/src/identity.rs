//! App identity: the display name, data directory and window/app IDs.

/// The name shown in window titles and the desktop launcher.
pub(crate) const APP_NAME: &str = "Bobble";
/// Directory under the platform data dir (XDG_DATA_HOME, LOCALAPPDATA, Application Support).
pub(crate) const DATA_DIR_NAME: &str = "Bobble";
/// Wayland app_id / X11 WM_CLASS base for the main window.
pub(crate) const APP_ID: &str = "io.github.fr_anz.bobble";
pub(crate) const MINION_CHAT_APP_ID: &str = "io.github.fr_anz.bobble.minion-chat";
pub(crate) const QUICK_COMPOSER_APP_ID: &str = "io.github.fr_anz.bobble.quick-composer";
