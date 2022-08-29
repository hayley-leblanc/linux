# Rust for Linux

The goal of this project is to add support for the Rust language to the Linux kernel. This repository contains the work that will be eventually submitted for review to the LKML.

Feel free to [contribute](https://github.com/Rust-for-Linux/linux/contribute)! To start, take a look at [`Documentation/rust`](https://github.com/Rust-for-Linux/linux/tree/rust/Documentation/rust).

General discussions, announcements, questions, etc. take place on the mailing list at rust-for-linux@vger.kernel.org ([subscribe](mailto:majordomo@vger.kernel.org?body=subscribe%20rust-for-linux), [instructions](http://vger.kernel.org/majordomo-info.html), [archive](https://lore.kernel.org/rust-for-linux/)). For chat, help, quick questions, informal discussion, etc. you may want to join our Zulip at https://rust-for-linux.zulipchat.com ([request an invitation](https://lore.kernel.org/rust-for-linux/CANiq72kW07hWjuc+dyvYH9NxyXoHsQLCtgvtR+8LT-VaoN1J_w@mail.gmail.com/T/)).

All contributors to this effort are understood to have agreed to the Linux kernel development process as explained in the different files under [`Documentation/process`](https://www.kernel.org/doc/html/latest/process/index.html).

<!-- XXX: Only for GitHub -- do not commit into mainline -->

# Rust FS setup instructions
TODO: come up with a nice name for it :)

## VM setup
TODO

## Kernel setup
TODO

Building the kernel: `make LLVM=1 -j n`

Installing the kernel: `sudo make modules modules_install install`

## Mounting the file system

To rebuild just the file system (not the whole kernel): `make LLVM=1 fs/hayleyfs/hayleyfs.ko`

To load the file system module: `make LLVM=1 fs/hayleyfs/hayleyfs.ko`

To mount the file system: `sudo mount -t hayleyfs -o init /dev/pmem0 /mnt/pmem`

## Running tests
### xfstests
Building: TODO
Running an individual test: `cd xfstests; sudo ./check generic/<test number>`