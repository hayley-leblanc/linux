#!/bin/bash

cwd=$(dirname "$0")

$cwd/load.sh

sudo mkdir /mnt/pmem/foo
sudo stat /mnt/pmem/foo

# sudo mkdir /mnt/pmem/bar
# sudo stat /mnt/pmem/bar

# sudo mkdir /mnt/pmem/foo/baz
# sudo stat /mnt/pmem/foo/baz

# sudo stat /mnt/pmem/foo

# sudo stat /mnt/pmem

$cwd/unload.sh