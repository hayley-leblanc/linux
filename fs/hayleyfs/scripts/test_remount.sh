cwd=$(dirname "$0")

$cwd/load.sh

sudo mkdir /mnt/pmem/foo
sudo umount /dev/pmem0

sudo mount -t hayleyfs /dev/pmem0 /mnt/pmem

$cwd/unload.sh