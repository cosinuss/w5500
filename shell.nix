let
  moz_overlay = import (builtins.fetchTarball https://github.com/mozilla/nixpkgs-mozilla/archive/master.tar.gz);
  nixpkgs = import <nixpkgs> { overlays = [ moz_overlay ]; };

  rust-channel = (nixpkgs.rustChannelOf { date = "2020-07-27"; channel = "nightly"; });
in
  with nixpkgs;
  stdenv.mkDerivation {
    name = "w5500";
    buildInputs = [
      rust-channel.rust
    ];
  }
