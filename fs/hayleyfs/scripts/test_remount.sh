cwd=$(dirname "$0")

sudo dmesg -C
sudo dd if=/dev/zero of=/dev/pmem0 bs=100M
sudo insmod fs/hayleyfs/hayleyfs.ko
sudo mount -t hayleyfs -o init,crash=3 /dev/pmem0 /mnt/pmem

sudo mkdir /mnt/pmem/foo
sudo umount /dev/pmem0

sudo mount -t hayleyfs /dev/pmem0 /mnt/pmem

$cwd/unload.sh