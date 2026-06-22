# RapidRAW GTK (relm4 port) — build/packaging.
# Named rapidraw-gtk / "RapidRAW GTK" so it never collides with the original
# Tauri app (RapidRAW / io.github.CyberTimon.RapidRAW).
#
# Targets:
#   make macos    -> dist/macos/RapidRAW GTK.app   (run on macOS, needs Homebrew gtk4+libadwaita)
#   make archpkg  -> rapidraw-gtk-*.pkg.tar.zst     (run on Arch, needs base-devel)

NAME      := RapidRAW GTK
BIN       := rapidraw-gtk
PKG       := rapidraw-gtk
APP_ID    := com.rapidraw.relm4
VERSION   := 0.1.0
MANIFEST  := rapidraw-relm4/Cargo.toml
RELEASE   := rapidraw-relm4/target/release/rapidraw-relm4
MACOS_APP := dist/macos/$(NAME).app
DMG       := dist/macos/$(NAME).dmg
DMG_STAGE := dist/macos/dmg

.PHONY: all build macos dmg archpkg clean

all: macos

build:
	cargo build --release --manifest-path $(MANIFEST)

# --- macOS .app -------------------------------------------------------------
# ponytail: not relocatable — links against Homebrew gtk4/libadwaita at their
# install paths. Self-contained bundle (gtk-mac-bundler + dylib relocation) is
# the upgrade path if you need to ship to machines without Homebrew.
macos: build
	rm -rf "$(MACOS_APP)"
	mkdir -p "$(MACOS_APP)/Contents/MacOS" "$(MACOS_APP)/Contents/Resources"
	cp "$(RELEASE)" "$(MACOS_APP)/Contents/MacOS/$(BIN)"
	cp src-tauri/icons/icon.icns "$(MACOS_APP)/Contents/Resources/icon.icns"
	printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0"><dict>' \
	  '  <key>CFBundleName</key><string>$(NAME)</string>' \
	  '  <key>CFBundleDisplayName</key><string>$(NAME)</string>' \
	  '  <key>CFBundleExecutable</key><string>$(BIN)</string>' \
	  '  <key>CFBundleIdentifier</key><string>$(APP_ID)</string>' \
	  '  <key>CFBundleIconFile</key><string>icon.icns</string>' \
	  '  <key>CFBundlePackageType</key><string>APPL</string>' \
	  '  <key>CFBundleShortVersionString</key><string>$(VERSION)</string>' \
	  '  <key>CFBundleVersion</key><string>$(VERSION)</string>' \
	  '  <key>NSHighResolutionCapable</key><true/>' \
	  '</dict></plist>' \
	  > "$(MACOS_APP)/Contents/Info.plist"
	xattr -rc "$(MACOS_APP)"
	codesign --force --deep --sign - "$(MACOS_APP)"
	@echo "Built $(MACOS_APP) (requires Homebrew gtk4 + libadwaita at runtime)"

# --- macOS .dmg installer ---------------------------------------------------
dmg: macos
	rm -rf "$(DMG_STAGE)"; mkdir -p "$(DMG_STAGE)"
	cp -R "$(MACOS_APP)" "$(DMG_STAGE)/"
	ln -sf /Applications "$(DMG_STAGE)/Applications"
	rm -f "$(DMG)"
	hdiutil create -volname "$(NAME)" -srcfolder "$(DMG_STAGE)" -ov -format UDZO "$(DMG)"
	rm -rf "$(DMG_STAGE)"
	@echo "Built $(DMG)"

# --- Arch Linux package -----------------------------------------------------
# ponytail: packages the prebuilt binary (install -U-able). A from-source
# PKGBUILD in a clean chroot / AUR is the upgrade path for distribution.
archpkg: build
	rm -rf dist/arch && mkdir -p dist/arch
	cp "$(RELEASE)" dist/arch/$(BIN)
	cp src-tauri/icons/128x128.png dist/arch/$(PKG).png
	printf '%s\n' \
	  '[Desktop Entry]' 'Type=Application' 'Name=$(NAME)' \
	  'Exec=$(BIN)' 'Icon=$(PKG)' 'Categories=Graphics;Photography;' \
	  'Terminal=false' \
	  > dist/arch/$(PKG).desktop
	printf '%s\n' \
	  'pkgname=$(PKG)' \
	  'pkgver=$(VERSION)' \
	  'pkgrel=1' \
	  "pkgdesc='RapidRAW GTK/relm4 port'" \
	  "arch=('x86_64' 'aarch64')" \
	  "url='https://github.com/CyberTimon/RapidRAW'" \
	  "license=('AGPL3')" \
	  "depends=('gtk4' 'libadwaita')" \
	  'package() {' \
	  '  install -Dm755 "$$startdir/$(BIN)" "$$pkgdir/usr/bin/$(BIN)"' \
	  '  install -Dm644 "$$startdir/$(PKG).desktop" "$$pkgdir/usr/share/applications/$(PKG).desktop"' \
	  '  install -Dm644 "$$startdir/$(PKG).png" "$$pkgdir/usr/share/icons/hicolor/128x128/apps/$(PKG).png"' \
	  '}' \
	  > dist/arch/PKGBUILD
	cd dist/arch && makepkg -f
	@echo "Built package in dist/arch/ — install with: sudo pacman -U dist/arch/$(PKG)-*.pkg.tar.zst"

clean:
	rm -rf dist
	cargo clean --manifest-path $(MANIFEST)
