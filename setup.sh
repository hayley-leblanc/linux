#!/bin/bash

if [ -z $1 ]
then 
    echo "Please specify number of cores for linux build"
    exit 1
fi

# install rust
echo "1" | curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup override set $(scripts/min-tool-version.sh rustc)
rustup component add rust-src 
cargo install --locked --version $(scripts/min-tool-version.sh bindgen) bindgen
rustup component add rustfmt
rustup component add clippy

sudo apt update
echo "y" | sudo apt install build-essential libncurses-dev bison flex libssl-dev \
    libelf-dev git openssh-server curl clang-14 zstd lld-14 llvm-14

cp CC_CONFIG .config
make LLVM=-14 -j $1
sudo make modules_install install

cd ..
git clone git@github.com:hayley-leblanc/fs-tests.git
git clone git@github.com:hayley-leblanc/filebench.git
cd filebench
libtoolize
aclocal
autoheader
automake --add-missing
autoconf 
./configure 
make
sudo make install 

sudo mkdir /mnt/pmem/

echo "Please update /etc/default/grub, run update-grub, and restart."
