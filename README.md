# Rust for Linux

The goal of this project is to add support for the Rust language to the Linux kernel. This repository contains the work that will be eventually submitted for review to the LKML.

Feel free to [contribute](https://github.com/Rust-for-Linux/linux/contribute)! To start, take a look at [`Documentation/rust`](https://github.com/Rust-for-Linux/linux/tree/rust/Documentation/rust).

General discussions, announcements, questions, etc. take place on the mailing list at rust-for-linux@vger.kernel.org ([subscribe](mailto:majordomo@vger.kernel.org?body=subscribe%20rust-for-linux), [instructions](http://vger.kernel.org/majordomo-info.html), [archive](https://lore.kernel.org/rust-for-linux/)). For chat, help, quick questions, informal discussion, etc. you may want to join our Zulip at https://rust-for-linux.zulipchat.com ([request an invitation](https://lore.kernel.org/rust-for-linux/CANiq72kW07hWjuc+dyvYH9NxyXoHsQLCtgvtR+8LT-VaoN1J_w@mail.gmail.com/T/)).

All contributors to this effort are understood to have agreed to the Linux kernel development process as explained in the different files under [`Documentation/process`](https://www.kernel.org/doc/html/latest/process/index.html).

<!-- XXX: Only for GitHub -- do not commit into mainline -->

# Rust FS setup instructions
TODO: come up with a nice name for it :)

## VM setup
Easiest way to run this right now is to create a big VM and do everything inside it. Building the Linux kernel requires a lot of space - my first VM grew to 70GB. 

TODO: figure out what the minimum size is
TODO: create a script for this

1. Create the VM image: `qemu-img create -f qcow2 <image name> <size>`.
2. Download Ubuntu 20.04 and boot the VM using `qemu-system-x86_64 -boot d -cdrom <path to ubuntu ISO> -m 8G -hda <image name> -enable-kvm`.
3. Follow the instructions in the graphical VM to install Ubuntu
4. Quit the VM and boot it again, this time using `qemu-system-x86_64 -boot c -m 8G -hda <image name> -enable-kvm`. 
5. Open a terminal in the graphical VM and run `sudo apt-get install build-essential libncurses-dev bison flex libssl-dev libelf-dev git openssh-server curl clang-11 lld-11 zstd`.
6. Fix symlinks so the correct versions are used: `cd /usr/bin; sudo ln -s clang-11 clang; sudo sudo ln -s ld.lld-11 ld.lld sudo ln -s ld.lld-11 ld.lld; sudo ln -s llvm-nm-11 llvm-nm; sudo ln -s llvm-objcopy-11 llvm-objcopy; sudo ln -s llvm-strip-11 llvm-strip; sudo ln -s llvm-objdump-11 llvm-objdump`

The VM can now be booted using `qemu-system-x86_64 -boot c -m <memory> -hda <image name> -enable-kvm -net nic -net user,hostfwd=tcp::2222-:22 -cpu host -nographic -smp <cores>` and accessed via `ssh` over port 2222. 

## Kernel setup
1. Clone the kernel using `git clone --filter=blob:none git@github.com:hayley-leblanc/linux.git`
2. Install Rust (see https://www.rust-lang.org/tools/install).
3. `cd linux` and follow the instructions here https://github.com/Rust-for-Linux/linux/blob/rust/Documentation/rust/quick-start.rst to install Rust dependencies. Currently, those steps are:
    1. `rustup override set $(scripts/min-tool-version.sh rustc)` to set the correct version of the Rust compiler
    2. `rustup component add rust-src` to obtain the Rust standard library source
    3. `cargo install --locked --version $(scripts/min-tool-version.sh bindgen) bindgen` to install bindgen, which is used to set up C bindings in the Rust part of the kernel.
    4. `rustup component add rustfmt` to install a tool to automatically format Rust code. IDEs will use this to format data if they are configured to run a formatter on save.
    5. `rustup component add clippy` to install the `clippy` linter
4. Run `yes "" | make config` to make a configuration file with the default options selected.
5. Ensure the `CONFIG_RUST` option (`General Setup -> Rust support`) is set to Y. If this option isn't available, make sure that `make LLVM=1 rustavailable` returns success and `CONFIG_MODVERSIONS` and `CONFIG_DEBUG_INFO_BTF` are disabled.
6. Set the following config options to avoid weird build issues:
    1. Set `CONFIG_SYSTEM_TRUSTED_KEYS` to an empty string
    2. Set `CONFIG_SYSTEM_REVOCATION_KEYS` to N
8. Build the kernel with `make LLVM=1 -j <number of cores>`. `LLVM=1` is necessary to build Rust components.

Installing the kernel: `sudo make modules modules_install install`

## Mounting the file system

To rebuild just the file system (not the whole kernel): `make LLVM=1 fs/hayleyfs/hayleyfs.ko`

To load the file system module: `make LLVM=1 fs/hayleyfs/hayleyfs.ko`

To mount the file system: `sudo mount -t hayleyfs -o init /dev/pmem0 /mnt/pmem`

## Running tests
### xfstests
Building: TODO
Running an individual test: `cd xfstests; sudo ./check generic/<test number>`
