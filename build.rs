fn main() {
    println!("cargo:rustc-link-lib=gtk-3");
    println!("cargo:rustc-link-lib=gdk-3");
    println!("cargo:rustc-link-lib=pangocairo-1.0");
    println!("cargo:rustc-link-lib=pango-1.0");
    println!("cargo:rustc-link-lib=fontconfig");
    println!("cargo:rustc-link-lib=cairo");
    println!("cargo:rustc-link-lib=gdk_pixbuf-2.0");
    println!("cargo:rustc-link-lib=gio-2.0");
    println!("cargo:rustc-link-lib=gobject-2.0");
    println!("cargo:rustc-link-lib=glib-2.0");
    println!("cargo:rustc-link-lib=dylib:+verbatim=libvte-2.91.so.0");
}
