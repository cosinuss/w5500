let
  moz_overlay = import (builtins.fetchTarball https://github.com/mozilla/nixpkgs-mozilla/archive/master.tar.gz);
  nixpkgs = import <nixpkgs> {
    overlays = [ moz_overlay ];
  };
in
  with nixpkgs;
  stdenv.mkDerivation {
    name = "hub";
    buildInputs = [
      ((rustChannelOf { date = "2020-03-28"; channel = "nightly"; }).rust.override {
        targets = [
          "x86_64-unknown-linux-gnu"
        ];
        extensions = [ "rust-src" "rust-std" ];
      })
    ];
  }

