# What crates/dassie-desktop builds against: GTK 3 and WebKitGTK 4.1, which
# Tauri's window and webview are on Linux, libsoup 3 for WebKit's network,
# and the AppIndicator library its tray icon loads at run time.
{ pkgs }:
{
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = [
    pkgs.gtk3
    pkgs.webkitgtk_4_1
    pkgs.libsoup_3
    pkgs.glib
    pkgs.libayatana-appindicator
  ];
}
