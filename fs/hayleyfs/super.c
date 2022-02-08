#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/dax.h>
#include <linux/pfn_t.h>

void hayleyfs_fs_put_dax(struct dax_device *dax_dev) {
    fs_put_dax(dax_dev);
}

unsigned long hayleyfs_pfn_t_to_pfn(pfn_t pfn) {
    return pfn_t_to_pfn(pfn);
}