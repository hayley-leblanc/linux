#include <linux/fs.h>
#include <linux/fs_context.h>
#include "super_rs.h"

// there are not proper bindings for some of the get tree stuff
// TODO: figure out how to set those bindings up
const struct fs_context_operations hayleyfs_context_ops = {
    .get_tree = hayleyfs_get_tree_rust,
};

int hayleyfs_get_tree(struct fs_context* fc) {
    return get_tree_bdev(fc, hayleyfs_fill_super);
}

void hayleyfs_fs_context_set_ops(struct fs_context *fc, const struct fs_context_operations *ops) {
    fc->ops = ops;
}

void hayleyfs_fs_context_set_fs_info(struct fs_context *fc, struct hayleyfs_fs_info *fsi) {
    fc->s_fs_info = fsi;
}