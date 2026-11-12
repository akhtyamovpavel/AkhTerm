use std::env;
use std::path::Path;
use std::process::Command;

const PKG_MODULES: &[&str] = &["gtk+-3.0", "vte-2.91", "fontconfig", "pango"];
const CORE_LIBS: &[&str] = &[
    "gtk-3",
    "gdk-3",
    "pangocairo-1.0",
    "pango-1.0",
    "fontconfig",
    "cairo",
    "gdk_pixbuf-2.0",
    "gio-2.0",
    "gobject-2.0",
    "glib-2.0",
];

fn pkg_config_bin() -> String {
    env::var("PKG_CONFIG")
        .ok()
        .filter(|val| !val.trim().is_empty())
        .unwrap_or_else(|| "pkg-config".to_owned())
}

fn push_unique_dir(dirs: &mut Vec<String>, candidate: &str) {
    let trimmed = candidate.trim();
    if trimmed.is_empty() || !Path::new(trimmed).is_dir() {
        return;
    }
    if !dirs.iter().any(|existing| existing == trimmed) {
        dirs.push(trimmed.to_owned());
    }
}

fn collect_pkg_config_dirs(bin: &str, dirs: &mut Vec<String>) {
    if let Ok(out) = Command::new(bin)
        .args(["--libs-only-L", "gtk+-3.0", "vte-2.91", "fontconfig"])
        .output()
    {
        if out.status.success() {
            parse_dash_l_flags(&String::from_utf8_lossy(&out.stdout), dirs);
        }
    }
    for pkg in PKG_MODULES {
        if let Ok(out) = Command::new(bin).args(["--libs-only-L", pkg]).output() {
            if out.status.success() {
                parse_dash_l_flags(&String::from_utf8_lossy(&out.stdout), dirs);
            }
        }
        if let Ok(out) = Command::new(bin).args(["--variable=libdir", pkg]).output() {
            if out.status.success() {
                push_unique_dir(dirs, &String::from_utf8_lossy(&out.stdout));
            }
        }
    }
}

fn parse_dash_l_flags(stdout: &str, dirs: &mut Vec<String>) {
    for token in stdout.split_whitespace() {
        if let Some(dir) = token.strip_prefix("-L") {
            push_unique_dir(dirs, dir);
        }
    }
}

fn candidate_system_libdirs(target_arch: &str) -> &'static [&'static str] {
    match target_arch {
        "x86_64" => &["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/lib64", "/usr/lib"],
        "aarch64" => &["/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/lib64", "/usr/lib"],
        _ => &[
            "/usr/lib64",
            "/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib",
        ],
    }
}

fn pkg_config_exists(bin: &str, pkg: &str) -> bool {
    Command::new(bin)
        .args(["--exists", pkg])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn has_lib_file(dirs: &[String], filename: &str) -> bool {
    dirs.iter().any(|dir| Path::new(dir).join(filename).exists())
}

fn emit_vte_link_directive(bin: &str, dirs: &[String]) {
    let has_unversioned = has_lib_file(dirs, "libvte-2.91.so");
    let has_soname = has_lib_file(dirs, "libvte-2.91.so.0");
    if has_unversioned || (pkg_config_exists(bin, "vte-2.91") && !has_soname) {
        println!("cargo:rustc-link-lib=vte-2.91");
    } else {
        println!("cargo:rustc-link-lib=dylib:+verbatim=libvte-2.91.so.0");
    }
}

fn git_short_sha() -> String {
    let Ok(out) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    else {
        return "release".to_owned();
    };
    if !out.status.success() {
        return "release".to_owned();
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if sha.is_empty() {
        "release".to_owned()
    } else {
        sha
    }
}

fn emit_git_rerun_hints() {
    let head = Path::new(".git/HEAD");
    if !head.exists() {
        return;
    }
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(content) = std::fs::read_to_string(head) {
        if let Some(rel) = content.trim().strip_prefix("ref: ") {
            let ref_path = Path::new(".git").join(rel.trim());
            if ref_path.exists() {
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
    }
}

fn resolve_build_target() -> String {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !arch.is_empty() && !os.is_empty() {
        return format!("{arch}-{os}");
    }
    env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    emit_git_rerun_hints();
    println!("cargo:rustc-env=AKHTERM_GIT_SHA={}", git_short_sha());
    println!("cargo:rustc-env=AKHTERM_BUILD_TARGET={}", resolve_build_target());
    for var in ["PKG_CONFIG", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let bin = pkg_config_bin();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let mut search_dirs = Vec::new();
    collect_pkg_config_dirs(&bin, &mut search_dirs);
    for dir in candidate_system_libdirs(&target_arch) {
        push_unique_dir(&mut search_dirs, dir);
    }

    for dir in &search_dirs {
        println!("cargo:rustc-link-search=native={dir}");
    }
    for lib in CORE_LIBS {
        println!("cargo:rustc-link-lib={lib}");
    }
    emit_vte_link_directive(&bin, &search_dirs);
}
