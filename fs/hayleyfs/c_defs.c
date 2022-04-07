#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/dax.h>
#include <linux/pfn_t.h>
#include <linux/bitops.h>
#include <linux/cred.h>
#include <linux/fs_parser.h>

void hayleyfs_fs_put_dax(struct dax_device *dax_dev) {
    fs_put_dax(dax_dev);
}

unsigned long hayleyfs_pfn_t_to_pfn(pfn_t pfn) {
    return pfn_t_to_pfn(pfn);
}

int hayleyfs_set_bit(int nr, void* addr) {
   return __test_and_set_bit_le(nr, addr);
}

void hayleyfs_clear_bit(int nr, void* addr) {
    return clear_bit_le(nr, addr);
}

unsigned long hayleyfs_find_next_zero_bit(const unsigned long *addr, unsigned long size, unsigned long offset) {
    return find_next_zero_bit_le(addr, size, offset);
}

int hayleyfs_test_bit(int nr, const void *addr) {
    return test_bit_le(nr, addr);
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

int hayleyfs_fs_parse(struct fs_context *fc,
		  const struct fs_parameter_spec *desc,
		  struct fs_parameter *param,
		  struct fs_parse_result *result)
{
	return fs_parse(fc, desc, param, result);
}

uid_t hayleyfs_uid_read(const struct inode *inode) {
    return i_uid_read(inode);
}

uid_t hayleyfs_gid_read(const struct inode *inode) {
    return i_gid_read(inode);
}

bool hayleyfs_isdir(uint16_t mode) {
    return S_ISDIR(mode);
}

bool hayleyfs_isreg(uint16_t mode) {
    return S_ISREG(mode);
}

void hayleyfs_write_uid(struct inode *inode, uid_t uid) {
    i_uid_write(inode, uid);
}

void hayleyfs_write_gid(struct inode *inode, gid_t gid) {
    i_gid_write(inode, gid);
}

void* hayleyfs_err_ptr(long error) {
    return ERR_PTR(error);
}

int hayleyfs_access_ok(const char* __user buf, size_t len) {
    return access_ok(buf, len);
}

unsigned long hayleyfs_copy_from_user_nt(void* dst, const void __user *src, unsigned long len) {
    return __copy_from_user_inatomic_nocache(dst, src, len);
}

unsigned long hayleyfs_copy_to_user(void __user *dst, const void *src, unsigned long len) {
    return copy_to_user(dst, src, len);
}

unsigned long hayleyfs_copy_from_user(void *dst, const void *src, unsigned long len) {
    return copy_from_user(dst, src, len);
}

void hayleyfs_i_size_write(struct inode *inode, loff_t i_size) {
    i_size_write(inode, i_size);
}

loff_t hayleyfs_i_size_read(const struct inode *inode) {
    return i_size_read(inode);
}