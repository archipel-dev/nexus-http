//! `cargo xtask dist`: release build, archives and native installers of `nexus`.
//!
//! Every run stages `dist/nexus-<version>-<os>-<arch>/` and packs it into
//! `.tar.gz` (`.zip` on Windows). It also builds the installers of the target OS:
//!
//! | OS | Artifacts | Tooling |
//! |---|---|---|
//! | Linux | `nexus_<version>_<debarch>.deb`, `nexus-<version>-linux-<arch>.AppImage` | `dpkg-deb`, `appimagetool` (`APPIMAGETOOL`) |
//! | macOS | `nexus-<version>-macos-<arch>.dmg` (unsigned `Nexus.app`, ad-hoc signed) | `codesign`, `hdiutil` |
//! | Windows | `nexus-<version>-windows-<arch>.msi` | WiX Toolset v5 (`wix`, `WIX_EXE`) |
//!
//! Each artifact gets a `<artifact>.sha256` in `sha256sum` format; artifact paths
//! are printed one per line (forward slashes). Optional icons are read from
//! `assets/icon/nexus{.png,.ico,.icns}` and skipped when absent.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const PACKAGE: &str = "nexus";
const APP_NAME: &str = "Nexus";
const BUNDLE_ID: &str = "dev.archipel.nexus";
const HOMEPAGE: &str = "https://github.com/archipel-dev/nexus-http";
const ICONS: &str = "assets/icon";
/// Stable across releases so MSI major upgrades replace older installs.
const WIX_UPGRADE_CODE: &str = "1fe8ec66-baa7-4abe-8552-d9a77806dbf8";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Rust target triple; the host when absent.
    pub target: Option<String>,
    /// Overrides the version read from the workspace `Cargo.toml`.
    pub version: Option<String>,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options> {
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--target" => {
                    options.target = Some(args.next().context("--target needs a triple")?)
                }
                "--version" => {
                    options.version = Some(args.next().context("--version needs a value")?)
                }
                other => bail!("unknown dist option `{other}`"),
            }
        }
        Ok(options)
    }
}

/// OS and architecture of a build, as used in artifact names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn host() -> Platform {
        Platform {
            os: match std::env::consts::OS {
                "macos" => "macos",
                "windows" => "windows",
                _ => "linux",
            }
            .into(),
            arch: std::env::consts::ARCH.into(),
        }
    }

    /// `x86_64-pc-windows-msvc` → windows / x86_64.
    pub fn from_triple(triple: &str) -> Result<Platform> {
        let arch = triple.split('-').next().unwrap_or_default();
        let os = if triple.contains("-windows") {
            "windows"
        } else if triple.contains("-apple-darwin") {
            "macos"
        } else if triple.contains("-linux") {
            "linux"
        } else {
            bail!("unsupported target `{triple}`")
        };
        if arch.is_empty() {
            bail!("unsupported target `{triple}`");
        }
        Ok(Platform {
            os: os.into(),
            arch: arch.into(),
        })
    }

    fn exe(&self) -> &'static str {
        if self.os == "windows" { ".exe" } else { "" }
    }

    /// `nexus-1.2.3-linux-x86_64`: the staging directory and archive stem.
    pub fn stem(&self, version: &str) -> String {
        format!("{PACKAGE}-{version}-{}-{}", self.os, self.arch)
    }
}

/// Reads `version` from the `[workspace.package]` table of a `Cargo.toml`.
pub fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_table = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_table = line == "[workspace.package]";
        } else if in_table && let Some(rest) = line.strip_prefix("version") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.split('"').nth(1)?.to_owned());
        }
    }
    None
}

/// `1.2.3-beta.1` → `1.2.3`: MSI and `CFBundleVersion` take numbers only.
pub fn numeric_version(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or(version)
}

/// Debian sorts `~` before the release, like SemVer pre-releases.
pub fn deb_version(version: &str) -> String {
    version.replacen('-', "~", 1)
}

pub fn deb_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn deb_file_name(version: &str, arch: &str) -> String {
    format!("{PACKAGE}_{}_{}.deb", deb_version(version), deb_arch(arch))
}

pub fn dist(options: Options) -> Result<()> {
    let version = match options.version {
        Some(version) => version,
        None => {
            let manifest = fs::read_to_string("Cargo.toml")
                .context("read Cargo.toml (run from the workspace root)")?;
            workspace_version(&manifest)
                .context("no version in the [workspace.package] table of Cargo.toml")?
        }
    };
    let platform = match &options.target {
        Some(triple) => Platform::from_triple(triple)?,
        None => Platform::host(),
    };
    let release_dir = build(options.target.as_deref())?;
    let name = platform.stem(&version);
    let dist = Path::new("dist");
    fs::create_dir_all(dist)?;
    let stage = dist.join(&name);
    stage_files(&release_dir, &stage, &platform)?;
    let mut artifacts = vec![if platform.os == "windows" {
        zip_dir(&stage, &dist.join(format!("{name}.zip")))?
    } else {
        tar_gz_dir(&stage, &dist.join(format!("{name}.tar.gz")))?
    }];
    match platform.os.as_str() {
        "windows" => artifacts.push(msi(&stage, &platform, &version, dist)?),
        "macos" => artifacts.push(dmg(&stage, &platform, &version, dist)?),
        _ => {
            artifacts.push(deb(&stage, &platform, &version, dist)?);
            artifacts.push(appimage(&stage, &platform, &version, dist)?);
        }
    }
    for artifact in &artifacts {
        write_checksum(artifact)?;
    }
    for artifact in &artifacts {
        println!("{}", artifact.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn build(target: Option<&str>) -> Result<PathBuf> {
    let metadata = cargo()
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .context("run cargo metadata")?;
    let text = String::from_utf8_lossy(&metadata.stdout);
    if !has_package(&text, PACKAGE) {
        bail!(
            "package `{PACKAGE}` does not exist in this workspace yet (expected crates/{PACKAGE}); \
             nothing to package"
        );
    }
    let mut command = cargo();
    command.args(["build", "--release", "-p", PACKAGE]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    run(&mut command, "cargo build")?;
    // Honour CARGO_TARGET_DIR / build.target-dir instead of assuming `target/`.
    let target_dir = json_string_field(&text, "target_directory")
        .map_or_else(|| PathBuf::from("target"), PathBuf::from);
    Ok(match target {
        Some(target) => target_dir.join(target).join("release"),
        None => target_dir.join("release"),
    })
}

/// Whether compact `cargo metadata` JSON lists a workspace package `name`.
pub fn has_package(metadata: &str, name: &str) -> bool {
    metadata.contains(&format!("{{\"name\":\"{name}\",\"version\""))
}

/// First string value of `"key":"..."` in compact JSON (no escapes handled
/// beyond `\\`), enough for `cargo metadata`'s `target_directory`.
pub fn json_string_field(json: &str, key: &str) -> Option<String> {
    let rest = json.split(&format!("\"{key}\":\"")).nth(1)?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

/// Lays out the portable distribution (also the payload of every installer).
fn stage_files(release_dir: &Path, out: &Path, platform: &Platform) -> Result<()> {
    if out.exists() {
        fs::remove_dir_all(out)?;
    }
    fs::create_dir_all(out)?;
    let exe = platform.exe();
    copy(
        &release_dir.join(format!("{PACKAGE}{exe}")),
        &out.join(format!("{PACKAGE}{exe}")),
    )?;
    for file in ["README.md", "LICENSE-APACHE", "LICENSE-MIT"] {
        copy(Path::new(file), &out.join(file))?;
    }
    if let Some(icon) = icon("png") {
        copy(&icon, &out.join(format!("{PACKAGE}.png")))?;
    }
    if platform.os == "linux" {
        fs::write(
            out.join(format!("{PACKAGE}.desktop")),
            desktop_entry(PACKAGE),
        )?;
    }
    Ok(())
}

/// `assets/icon/nexus.<ext>` when it exists.
fn icon(ext: &str) -> Option<PathBuf> {
    let path = Path::new(ICONS).join(format!("{PACKAGE}.{ext}"));
    path.is_file().then_some(path)
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).with_context(|| format!("copy {}", from.display()))?;
    Ok(())
}

fn run(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("run {what} (is it installed?)"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

/// Freedesktop launcher; `exec` is `nexus` (on PATH) or an absolute path.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={APP_NAME}\n\
         GenericName=API Client\n\
         Comment=Native HTTP API client\n\
         Exec={exec}\n\
         Icon={PACKAGE}\n\
         Terminal=false\n\
         Categories=Development;Network;\n\
         Keywords=HTTP;API;REST;Client;\n\
         StartupWMClass={PACKAGE}\n"
    )
}

/// `sha256sum`-compatible `<artifact>.sha256`.
fn write_checksum(artifact: &Path) -> Result<()> {
    let bytes = fs::read(artifact).with_context(|| format!("read {}", artifact.display()))?;
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let name = artifact
        .file_name()
        .context("artifact has no file name")?
        .to_string_lossy();
    let mut path = artifact.as_os_str().to_owned();
    path.push(".sha256");
    fs::write(&path, format!("{digest}  {name}\n"))?;
    Ok(())
}

fn tar_gz_dir(dir: &Path, archive: &Path) -> Result<PathBuf> {
    let file = fs::File::create(archive)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Deterministic);
    builder.follow_symlinks(false);
    builder.append_dir_all(dir.file_name().context("archive root")?, dir)?;
    builder.into_inner()?.finish()?.flush()?;
    Ok(archive.to_owned())
}

fn zip_dir(dir: &Path, archive: &Path) -> Result<PathBuf> {
    let root = dir.file_name().context("archive root")?.to_string_lossy();
    let mut zip = zip::ZipWriter::new(fs::File::create(archive)?);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for file in files_under(dir)? {
        let relative = file.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
        zip.start_file(format!("{root}/{relative}"), options)?;
        zip.write_all(&fs::read(&file)?)?;
    }
    zip.finish()?;
    Ok(archive.to_owned())
}

/// Regular files below `dir`, sorted for reproducible archives.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(std::env::current_dir()?.join(path))
}

// ---------------------------------------------------------------- Windows

/// WiX v5 source: per-machine install into `Program Files\Nexus` plus a Start
/// Menu shortcut. `$(Version)` and `$(Stage)` are passed with `wix build -d`.
pub fn wix_source() -> String {
    format!(
        r#"<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package Name="{APP_NAME}" Manufacturer="Archipel" Version="$(Version)" UpgradeCode="{WIX_UPGRADE_CODE}" Scope="perMachine">
    <MajorUpgrade DowngradeErrorMessage="A newer version of {APP_NAME} is already installed." />
    <MediaTemplate EmbedCab="yes" />
    <StandardDirectory Id="ProgramFiles64Folder">
      <Directory Id="INSTALLFOLDER" Name="{APP_NAME}">
        <Component Id="MainExecutable">
          <File Id="NexusExe" Source="$(Stage)\{PACKAGE}.exe" KeyPath="yes">
            <Shortcut Id="StartMenuShortcut" Directory="ProgramMenuFolder" Name="{APP_NAME}" WorkingDirectory="INSTALLFOLDER" Advertise="yes" />
          </File>
        </Component>
        <Component Id="Readme">
          <File Id="Readme" Source="$(Stage)\README.md" KeyPath="yes" />
        </Component>
        <Component Id="LicenseApache">
          <File Id="LicenseApache" Source="$(Stage)\LICENSE-APACHE" KeyPath="yes" />
        </Component>
        <Component Id="LicenseMit">
          <File Id="LicenseMit" Source="$(Stage)\LICENSE-MIT" KeyPath="yes" />
        </Component>
      </Directory>
    </StandardDirectory>
    <Feature Id="Main" Title="{APP_NAME}" Level="1">
      <ComponentRef Id="MainExecutable" />
      <ComponentRef Id="Readme" />
      <ComponentRef Id="LicenseApache" />
      <ComponentRef Id="LicenseMit" />
    </Feature>
  </Package>
</Wix>
"#
    )
}

fn msi(stage: &Path, platform: &Platform, version: &str, dist: &Path) -> Result<PathBuf> {
    let arch = match platform.arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("no MSI architecture for {other}"),
    };
    let source = dist.join("nexus.wxs");
    fs::write(&source, wix_source())?;
    let out = dist.join(format!("{}.msi", platform.stem(version)));
    let wix = std::env::var("WIX_EXE").unwrap_or_else(|_| "wix".into());
    run(
        Command::new(wix)
            .args(["build", "-nologo", "-arch", arch])
            .arg("-d")
            .arg(format!("Version={}", numeric_version(version)))
            .arg("-d")
            .arg(format!("Stage={}", absolute(stage)?.display()))
            .arg("-o")
            .arg(&out)
            .arg(&source),
        "wix build (install it with `dotnet tool install --global wix`)",
    )?;
    fs::remove_file(&source)?;
    Ok(out)
}

// ---------------------------------------------------------------- macOS

pub fn info_plist(version: &str) -> String {
    let short = numeric_version(version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>{APP_NAME}</string>
  <key>CFBundleExecutable</key><string>{PACKAGE}</string>
  <key>CFBundleIconFile</key><string>{PACKAGE}</string>
  <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>{APP_NAME}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{short}</string>
  <key>CFBundleVersion</key><string>{short}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>{APP_NAME} {version}. Apache-2.0 OR MIT.</string>
</dict>
</plist>
"#
    )
}

/// `Nexus.app` inside a compressed `.dmg` with an Applications link. Signed ad
/// hoc, or with `NEXUS_MACOS_SIGN_IDENTITY` when set.
fn dmg(stage: &Path, platform: &Platform, version: &str, dist: &Path) -> Result<PathBuf> {
    let image_root = dist.join(format!("dmg-{}", platform.arch));
    if image_root.exists() {
        fs::remove_dir_all(&image_root)?;
    }
    let app = image_root.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::write(contents.join("Info.plist"), info_plist(version))?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;
    copy(&stage.join(PACKAGE), &macos.join(PACKAGE))?;
    if let Some(icns) = icon("icns") {
        copy(&icns, &resources.join(format!("{PACKAGE}.icns")))?;
    }
    for file in ["README.md", "LICENSE-APACHE", "LICENSE-MIT"] {
        copy(&stage.join(file), &resources.join(file))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", image_root.join("Applications"))?;

    let identity = std::env::var("NEXUS_MACOS_SIGN_IDENTITY")
        .ok()
        .filter(|identity| !identity.is_empty())
        .unwrap_or_else(|| "-".into());
    run(
        Command::new("codesign")
            .args(["--force", "--deep", "--sign", &identity])
            .arg(&app),
        "codesign",
    )?;

    let out = dist.join(format!("{}.dmg", platform.stem(version)));
    let _ = fs::remove_file(&out);
    // hdiutil intermittently fails with "Resource busy" on CI runners.
    let mut attempt = 1;
    loop {
        let result = run(
            Command::new("hdiutil")
                .args(["create", "-volname", &format!("{APP_NAME} {version}")])
                .arg("-srcfolder")
                .arg(&image_root)
                .args(["-ov", "-format", "UDZO"])
                .arg(&out),
            "hdiutil create",
        );
        match result {
            Ok(()) => return Ok(out),
            Err(error) if attempt >= 3 => return Err(error),
            Err(error) => {
                eprintln!("{error}; retrying");
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        }
    }
}

// ---------------------------------------------------------------- Linux

pub fn deb_control(version: &str, arch: &str, installed_kib: u64, glibc: Option<&str>) -> String {
    let libc = glibc.map_or_else(|| "libc6".to_owned(), |v| format!("libc6 (>= {v})"));
    format!(
        "Package: {PACKAGE}\n\
         Version: {}\n\
         Section: devel\n\
         Priority: optional\n\
         Architecture: {}\n\
         Installed-Size: {installed_kib}\n\
         Depends: {libc}, libgcc-s1\n\
         Recommends: libgl1, libxkbcommon0, libvulkan1, libwayland-client0 | libx11-6, xdg-utils\n\
         Maintainer: Archipel contributors <{HOMEPAGE}/issues>\n\
         Homepage: {HOMEPAGE}\n\
         Description: Native HTTP API client\n \
         {APP_NAME} is a fast, local-first HTTP API client of the Archipel suite.\n",
        deb_version(version),
        deb_arch(arch),
    )
}

/// Highest `GLIBC_x.y` symbol version the binary needs, via `objdump -T`.
pub fn required_glibc(objdump_output: &str) -> Option<String> {
    objdump_output
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter_map(|word| word.strip_prefix("GLIBC_"))
        .filter_map(|version| {
            let parts: Option<Vec<u32>> = version.split('.').map(|p| p.parse().ok()).collect();
            parts.map(|parts| (parts, version.to_owned()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version)| version)
}

/// `/usr/bin/nexus`, desktop entry and optional icon.
fn deb(stage: &Path, platform: &Platform, version: &str, dist: &Path) -> Result<PathBuf> {
    let root = dist.join(format!("deb-{}", platform.arch));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    let bin = root.join("usr/bin");
    fs::create_dir_all(&bin)?;
    copy(&stage.join(PACKAGE), &bin.join(PACKAGE))?;
    let applications = root.join("usr/share/applications");
    fs::create_dir_all(&applications)?;
    fs::write(
        applications.join(format!("{PACKAGE}.desktop")),
        desktop_entry(PACKAGE),
    )?;
    if let Some(png) = icon("png") {
        let dir = root.join("usr/share/icons/hicolor/512x512/apps");
        fs::create_dir_all(&dir)?;
        copy(&png, &dir.join(format!("{PACKAGE}.png")))?;
    }
    let doc = root.join(format!("usr/share/doc/{PACKAGE}"));
    fs::create_dir_all(&doc)?;
    for license in ["LICENSE-APACHE", "LICENSE-MIT"] {
        copy(&stage.join(license), &doc.join(license))?;
    }
    fs::write(
        doc.join("copyright"),
        format!(
            "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\
             Upstream-Name: {PACKAGE}\nSource: {HOMEPAGE}\n\n\
             Files: *\nLicense: Apache-2.0 OR MIT\n"
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Normalise modes regardless of the builder's umask.
        for file in files_under(&root)? {
            let mode = if file == bin.join(PACKAGE) {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&file, fs::Permissions::from_mode(mode))?;
        }
    }
    let installed_bytes: u64 = files_under(&root)?
        .iter()
        .filter_map(|file| fs::metadata(file).ok())
        .map(|meta| meta.len())
        .sum();
    let glibc = Command::new("objdump")
        .arg("-T")
        .arg(bin.join(PACKAGE))
        .output()
        .ok()
        .and_then(|output| required_glibc(&String::from_utf8_lossy(&output.stdout)));
    let debian = root.join("DEBIAN");
    fs::create_dir_all(&debian)?;
    fs::write(
        debian.join("control"),
        deb_control(
            version,
            &platform.arch,
            installed_bytes.div_ceil(1024),
            glibc.as_deref(),
        ),
    )?;
    let out = dist.join(deb_file_name(version, &platform.arch));
    run(
        Command::new("dpkg-deb")
            .args(["--root-owner-group", "-Zxz", "--build"])
            .arg(&root)
            .arg(&out),
        "dpkg-deb",
    )?;
    Ok(out)
}

/// Portable single-file build: an AppDir packed by `appimagetool` (path in
/// `APPIMAGETOOL`, else on PATH).
fn appimage(stage: &Path, platform: &Platform, version: &str, dist: &Path) -> Result<PathBuf> {
    let app_dir = dist.join(format!("AppDir-{}", platform.arch));
    if app_dir.exists() {
        fs::remove_dir_all(&app_dir)?;
    }
    let bin = app_dir.join("usr/bin");
    fs::create_dir_all(&bin)?;
    copy(&stage.join(PACKAGE), &bin.join(PACKAGE))?;
    let desktop = desktop_entry(PACKAGE);
    let applications = app_dir.join("usr/share/applications");
    fs::create_dir_all(&applications)?;
    fs::write(applications.join(format!("{PACKAGE}.desktop")), &desktop)?;
    fs::write(app_dir.join(format!("{PACKAGE}.desktop")), &desktop)?;
    // appimagetool requires the `Icon=` file at the AppDir root.
    match icon("png") {
        Some(png) => copy(&png, &app_dir.join(format!("{PACKAGE}.png")))?,
        None => fs::write(app_dir.join(format!("{PACKAGE}.png")), PLACEHOLDER_PNG)?,
    }
    fs::write(app_dir.join("AppRun"), app_run())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        fs::set_permissions(app_dir.join("AppRun"), fs::Permissions::from_mode(0o755))?;
        symlink(format!("{PACKAGE}.png"), app_dir.join(".DirIcon"))?;
    }
    let out = dist.join(format!("{}.AppImage", platform.stem(version)));
    let _ = fs::remove_file(&out);
    let tool = std::env::var("APPIMAGETOOL").unwrap_or_else(|_| "appimagetool".into());
    run(
        Command::new(tool)
            // Runs without FUSE (containers, CI).
            .env("APPIMAGE_EXTRACT_AND_RUN", "1")
            .env("ARCH", platform.arch.as_str())
            .arg("--no-appstream")
            .arg(&app_dir)
            .arg(&out),
        "appimagetool",
    )?;
    Ok(out)
}

/// Starts the bundled app; `APPDIR` is set by the AppImage runtime.
pub fn app_run() -> String {
    format!(
        "#!/bin/sh\nHERE=\"${{APPDIR:-$(dirname \"$(readlink -f \"$0\")\")}}\"\nexec \"$HERE/usr/bin/{PACKAGE}\" \"$@\"\n"
    )
}

/// 1x1 transparent PNG used until the project ships `assets/icon/nexus.png`.
const PLACEHOLDER_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35, 0x81, 0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options> {
        Options::parse(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn parses_options() {
        assert_eq!(parse(&[]).unwrap(), Options::default());
        assert_eq!(
            parse(&["--target", "x86_64-apple-darwin", "--version", "1.0.0"]).unwrap(),
            Options {
                target: Some("x86_64-apple-darwin".into()),
                version: Some("1.0.0".into()),
            }
        );
        assert!(parse(&["--target"]).is_err());
        assert!(parse(&["--version"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }

    #[test]
    fn maps_target_triples_to_release_names() {
        let platform = |triple| Platform::from_triple(triple).unwrap();
        assert_eq!(
            platform("x86_64-pc-windows-msvc"),
            Platform {
                os: "windows".into(),
                arch: "x86_64".into()
            }
        );
        assert_eq!(platform("aarch64-apple-darwin").os, "macos");
        assert_eq!(platform("x86_64-apple-darwin").arch, "x86_64");
        assert_eq!(platform("aarch64-unknown-linux-gnu").os, "linux");
        assert!(Platform::from_triple("wasm32-unknown-unknown").is_err());
    }

    #[test]
    fn names_artifacts() {
        let linux = Platform::from_triple("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(linux.stem("0.1.0"), "nexus-0.1.0-linux-x86_64");
        assert_eq!(deb_file_name("0.1.0", "x86_64"), "nexus_0.1.0_amd64.deb");
        assert_eq!(
            deb_file_name("0.2.0-beta.1", "aarch64"),
            "nexus_0.2.0~beta.1_arm64.deb"
        );
    }

    #[test]
    fn reads_workspace_version() {
        let manifest = "[workspace]\nmembers = []\n\n[package]\nversion = \"9.9.9\"\n\n\
                        [workspace.package]\nedition = \"2024\"\nversion = \"0.3.1\" # x\n";
        assert_eq!(workspace_version(manifest).as_deref(), Some("0.3.1"));
        assert_eq!(workspace_version("[workspace]\n"), None);
    }

    #[test]
    fn reads_json_string_fields() {
        let json = r#"{"packages":[],"target_directory":"/tmp/a\\b","x":1}"#;
        assert_eq!(
            json_string_field(json, "target_directory").as_deref(),
            Some("/tmp/a\\b")
        );
        assert_eq!(json_string_field(json, "missing"), None);
    }

    #[test]
    fn detects_workspace_packages() {
        let json = r#"{"packages":[{"name":"xtask","version":"0.0.0"}]}"#;
        assert!(has_package(json, "xtask"));
        assert!(!has_package(json, "nexus"));
    }

    #[test]
    fn versions_fit_each_package_format() {
        assert_eq!(numeric_version("0.0.1"), "0.0.1");
        assert_eq!(numeric_version("1.2.0-beta.1"), "1.2.0");
        assert_eq!(deb_version("1.2.0-beta.1"), "1.2.0~beta.1");
        assert_eq!(deb_arch("x86_64"), "amd64");
        assert_eq!(deb_arch("aarch64"), "arm64");
    }

    #[test]
    fn deb_control_declares_package_metadata() {
        let control = deb_control("0.1.0", "x86_64", 4242, Some("2.34"));
        assert!(control.contains("Package: nexus\n"));
        assert!(control.contains("Version: 0.1.0\n"));
        assert!(control.contains("Architecture: amd64\n"));
        assert!(control.contains("Installed-Size: 4242\n"));
        assert!(control.contains("Depends: libc6 (>= 2.34), libgcc-s1\n"));
        let description = control.split("Description:").nth(1).unwrap();
        assert!(
            description
                .lines()
                .skip(1)
                .all(|line| line.starts_with(' '))
        );
        assert!(control.ends_with('\n'));
    }

    #[test]
    fn finds_the_newest_glibc_symbol_version() {
        let objdump = "0000 DF *UND* 0000 (GLIBC_2.2.5) memcpy\n\
                       0000 DF *UND* 0000 (GLIBC_2.34) __libc_start_main\n\
                       0000 DF *UND* 0000 (GLIBC_2.9) pipe2\n\
                       0000 DF *UND* 0000 (GLIBC_PRIVATE) x\n";
        assert_eq!(required_glibc(objdump).as_deref(), Some("2.34"));
        assert_eq!(required_glibc("nothing"), None);
    }

    #[test]
    fn bundle_and_launcher_metadata() {
        let plist = info_plist("1.2.0-beta.1");
        assert!(plist.contains("<key>CFBundleExecutable</key><string>nexus</string>"));
        assert!(plist.contains("<key>CFBundleName</key><string>Nexus</string>"));
        assert!(plist.contains("<string>dev.archipel.nexus</string>"));
        assert!(plist.contains("<key>CFBundleVersion</key><string>1.2.0</string>"));
        let desktop = desktop_entry("nexus");
        assert!(desktop.starts_with("[Desktop Entry]\n"));
        assert!(desktop.contains("Name=Nexus\n"));
        assert!(desktop.contains("Exec=nexus\n"));
        assert!(desktop.contains("Icon=nexus\n"));
        assert!(app_run().contains("exec \"$HERE/usr/bin/nexus\""));
    }

    #[test]
    fn wix_source_is_a_per_machine_package_with_shortcut() {
        let wxs = wix_source();
        assert!(wxs.contains("Name=\"Nexus\""));
        assert!(wxs.contains(WIX_UPGRADE_CODE));
        assert!(wxs.contains("Source=\"$(Stage)\\nexus.exe\""));
        assert!(wxs.contains("<Shortcut"));
        assert!(wxs.contains("<MajorUpgrade"));
        // WiX derives component GUIDs only for single-file components here: none of
        // the files carries a version resource (error WIX0367 otherwise).
        assert_eq!(
            wxs.matches("<Component ").count(),
            wxs.matches("<File ").count(),
            "one file per component"
        );
        assert_eq!(
            wxs.matches("<Component ").count(),
            wxs.matches("<ComponentRef ").count(),
            "every component is in the feature"
        );
    }

    #[test]
    fn checksum_files_use_sha256sum_format() {
        let dir = std::env::temp_dir().join(format!("xtask-sum-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("nexus.tar.gz");
        fs::write(&file, b"abc").unwrap();
        write_checksum(&file).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("nexus.tar.gz.sha256")).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  nexus.tar.gz\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
