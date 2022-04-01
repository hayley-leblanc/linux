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

uint64_t hayleyfs_cpu_to_le64_unsafe(uint64_t val) {
    return cpu_to_le64(val);
}

int64_t hayleyfs_cpu_to_le64_signed_unsafe(int64_t val) {
    return cpu_to_le64(val);
}

uint32_t hayleyfs_cpu_to_le32_unsafe(uint32_t val) {
    return cpu_to_le32(val);
}

uint16_t hayleyfs_cpu_to_le16_unsafe(uint16_t val) {
    return cpu_to_le16(val);
}

bool hayleyfs_isdir(uint16_t flags) {
    return S_ISDIR(flags);
}

bool hayleyfs_isreg(uint16_t flags) {
    return S_ISREG(flags);
}