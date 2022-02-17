#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/dax.h>
#include <linux/pfn_t.h>
#include <linux/bitops.h>
#include <linux/cred.h>

void hayleyfs_fs_put_dax(struct dax_device *dax_dev) {
    fs_put_dax(dax_dev);
}

unsigned long hayleyfs_pfn_t_to_pfn(pfn_t pfn) {
    return pfn_t_to_pfn(pfn);
}

int hayleyfs_set_bit(int nr, void* addr) {
   return __test_and_set_bit_le(nr, addr);
}

unsigned long hayleyfs_find_next_zero_bit(const unsigned long *addr, unsigned long size, unsigned long offset) {
    return find_next_zero_bit_le(addr, size, offset);
}

bool hayleyfs_dir_emit(struct dir_context* ctx, const char *name, int namelen, u64 ino, unsigned type) {
    return dir_emit(ctx, name, namelen, ino, type);   
}

struct inode* hayleyfs_file_inode(const struct file *f) {
    return file_inode(f);
}

kuid_t hayleyfs_current_fsuid(void) {
    return current_fsuid();
}

kgid_t hayleyfs_current_fsgid(void) {
    return current_fsgid();
}