%global debug_package %{nil}
%global _docdir_fmt %{name}

Name:           akhterm
Version:        %{pkg_version}
Release:        1
Summary:        Native Linux terminal with SSH, remote tmux, and automatic tab restoration
License:        MIT and OFL-1.1
URL:            https://github.com/akhtyamovpavel/AkhTerm

Requires:       gtk3, vte291, librsvg2, openssh-clients
Recommends:     xdg-utils, libnotify
Suggests:       htop, wl-clipboard

%description
AkhTerm is a native Linux terminal built with Rust, GTK 3, and VTE 2.91.
It keeps local shells, SSH connections, and remote tmux sessions in one window
and restores their layout, pinned titles, split panes, and working directories.

%prep
%setup -q -c -T
install -Dm0644 "%{repo_root}/config.example.conf" ./config.example.conf
install -Dm0644 "%{repo_root}/LICENSE" ./LICENSE
install -Dm0644 "%{repo_root}/assets/fonts/OFL-Geist.txt" ./assets/fonts/OFL-Geist.txt
install -Dm0644 "%{repo_root}/packaging/NOTICE" ./packaging/NOTICE

%build
# Built offline via cargo in packaging/build-rpm.sh.

%install
install -Dm0755 "%{repo_root}/target/release/akhterm" "%{buildroot}%{_bindir}/akhterm"
install -Dm0644 "%{repo_root}/packaging/org.akhterm.AkhTerm.desktop" \
    "%{buildroot}%{_datadir}/applications/org.akhterm.AkhTerm.desktop"
install -Dm0644 "%{repo_root}/packaging/org.akhterm.AkhTerm.svg" \
    "%{buildroot}%{_datadir}/icons/hicolor/scalable/apps/org.akhterm.AkhTerm.svg"

%files
%attr(0755,root,root) %{_bindir}/akhterm
%attr(0644,root,root) %{_datadir}/applications/org.akhterm.AkhTerm.desktop
%attr(0644,root,root) %{_datadir}/icons/hicolor/scalable/apps/org.akhterm.AkhTerm.svg
%doc config.example.conf
%license LICENSE assets/fonts/OFL-Geist.txt packaging/NOTICE
