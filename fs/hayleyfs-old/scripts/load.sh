#!/bin/bash

sudo dmesg -C
sudo dd if=/dev/zero of=/dev/pmem0 bs=100M
sudo insmod fs/hayleyfs/hayleyfs.ko
sudo mount -t hayleyfs -o init /dev/pmem0 /mnt/pmem