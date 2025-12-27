# Maintainer: Alessio Deiana <adeiana@gmail.com>
pkgname=coolctl
pkgver=0.2.0
pkgrel=1
pkgdesc="CPU thermal throttle daemon"
arch=('x86_64')
license=('MIT')
depends=('gcc-libs')
makedepends=('cargo')
backup=('etc/coolctl.toml')

build() {
  cd "$startdir"
  cargo build --release
}

package() {
  cd "$startdir"
  install -Dm755 "target/release/coolctl" "$pkgdir/usr/bin/coolctl"
  install -Dm644 "coolctl.service" "$pkgdir/usr/lib/systemd/system/coolctl.service"
  install -Dm644 "coolctl.toml.example" "$pkgdir/etc/coolctl.toml"
  install -Dm644 "SPEC.md" "$pkgdir/usr/share/doc/coolctl/SPEC.md"
}
