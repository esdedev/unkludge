//! On Windows with MSVC, embeds the app's icon and manifest in the executable. The icon is drawn by
//! the same code as the tray icon (src/icon.rs) and both are written as a compiled resource
//! (src/winres.rs), which the MSVC linker takes as it is. No external tools.

#[path = "src/icon.rs"]
mod icon;
#[path = "src/winres.rs"]
mod winres;

use std::path::Path;

/// Small sizes as bitmaps. The largest, for the large icons view, as a compressed PNG, so that the
/// executable doesn't grow by it.
const DIB_SIZES: [usize; 6] = [16, 20, 24, 32, 48, 64];
const PNG_SIZE: usize = 256;

/// Asks for the current look of the system's dialogs, which otherwise come out as in Windows 2000,
/// and says the app handles each monitor's scale by itself.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=src/winres.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV");
    if target_os.as_deref() != Ok("windows") {
        return;
    }
    if target_env.as_deref() != Ok("msvc") {
        println!("cargo:warning=the executable gets no icon: it's only embedded with MSVC");
        return;
    }
    let images: Vec<(usize, Vec<u8>)> = DIB_SIZES
        .iter()
        .map(|&size| (size, winres::dib(size, &icon::pixels(size))))
        .chain(std::iter::once((
            PNG_SIZE,
            winres::png(PNG_SIZE, PNG_SIZE, &icon::pixels(PNG_SIZE)),
        )))
        .collect();
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let path = Path::new(&out).join("unkludge.res");
    std::fs::write(&path, winres::res(&images, MANIFEST))
        .expect("the icon's resource can't be written");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
