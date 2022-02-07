#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/dax.h>
// #include "super_rs.h"

// // there are not proper bindings for some of the get tree stuff
// // TODO: figure out how to set those bindings up
// const struct fs_context_operations hayleyfs_context_ops = {
//     .get_tree = hayleyfs_get_tree_rust,
// };

// int hayleyfs_get_tree(struct fs_context* fc) {
//     return get_tree_bdev(fc, hayleyfs_fill_super);
// }

// void hayleyfs_fs_context_set_fs_info(struct fs_context *fc, struct hayleyfs_fs_info *fsi) {
//     fc->s_fs_info = fsi;
// }

// void* hayleyfs_fs_context_get_fs_info(struct fs_context *fc) {
//     return fc->s_fs_info;
// }

void hayleyfs_fs_put_dax(struct dax_device *dax_dev) {
    fs_put_dax(dax_dev);
}