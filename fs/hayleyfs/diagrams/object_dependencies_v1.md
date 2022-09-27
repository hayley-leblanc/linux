# Object Dependencies v1

Round nodes represent operations. Gray nodes indicate operations on DRAM specifically. Hexagon nodes indicate dependencies on other persistent objects. If multiple objects must be persistent before an operation, they will be listed in the same node; separate nodes indicate that there are multiple dependencies, only one of which must be satisfied. The dependencies here represent explicit persisted-before relationships and each object must be flushed/fenced before dependent operations can take place, so we only include soft updates operation typestate here (not persistence typestate).

**Inode initialization**

```mermaid
graph TD

dram_alloc([Allocate inode])
free_inode["Inode(Free)"]
pm_alloc([Alloc inode on PM])
alloc_inode["Inode(Alloc)"]

dram_alloc --> free_inode
free_inode --> pm_alloc
pm_alloc --> alloc_inode

classDef empty width:0px,height:0px;
classDef gray fill:#888,stroke:#333,stroke-width:2px;
class dram_alloc gray
```
Allocating an inode on PM involves setting all of the fields in its first 64 bytes. Right now, inodes are smaller than 64 bytes; if they grow larger, we will need to adjust this diagram to account for flushing the subsequent bytes.

Safety:
- It's always safe to allocate an inode in an empty (completely zeroed out) slot; as long as no directory entry refers to it, doing so will just cause a space leak, even if the inode is fully initialized.

**Inode modification and deletion**

```mermaid
graph TD

get_inode([Get inode])
start_inode["Inode(Start)"]
inc_link([Increment link count])
inc_link_inode["Inode(IncLink)"]
inc_size([Increase size])
inc_size_inode["Inode(IncSize)"]
dec_size([Decrease size])
dec_size_inode["Inode(DecSize)"]
dec_links([Decrement link count])
dec_link_inode["Inode(DecLink)"]
dealloc([Deallocate])
dealloc_inode["Inode(Dealloc)"]

dataph_write{{"DataPageHeader(Write)"}}
dentry_clear{{"Dentry(ClearIno)"}}

get_inode --> start_inode
start_inode --> inc_link --> inc_link_inode
start_inode --> inc_size --> inc_size_inode
dataph_write -.-> inc_size
start_inode --> dec_size --> dec_size_inode
start_inode --> dec_links --> dec_link_inode
dentry_clear --> dec_links
start_inode --> dealloc --> dealloc_inode
dentry_clear -.-> dealloc



classDef empty width:0px,height:0px;
classDef gray fill:#888,stroke:#333,stroke-width:2px;
class get_inode gray
```
Safety:
- It's always safe to increase link count, since too-high link count is a valid space leak
- File size cannot be increased until there are data page(s) with that much data written to them
- It's always safe to decrease file size, since doing so can only cause space leaks
- Link count cannot be decremented until the corresponding dentry has been invalidated by clearing its ino field; a too-low link count is an illegal inconsistency
- Inode cannot be deallocated until the corresponding dentry has been invalidated by clearing its ino field; otherwise we risk a dangling pointer from the dentry to the deallocated inode or a violation of rule 2 (nullify all pointers to a resource before reusing it).

**Dentry initialization**
```mermaid
graph TD

dram_alloc([Find free dentry])
get_old_dentry([Find old dentry])
free_dentry["Dentry(Free)"]
pm_alloc([Allocate dentry on PM])
alloc_dentry["Dentry(Alloc)"]
init([Set ino])
init_dentry["Dentry(Init)"]
alloc_bkptr([Allocate on PM with backpointer])
alloc_bkptr_dentry["Dentry(SetBackpointer)"]
init_bkptr([Set ino])
init_bkptr_dentry["Dentry(InitBackpointer)"]
clear_bkptr([Clear backpointer])
clear_bkptr_dentry["Dentry(ClearBackpointer)"]

inode_alloc{{"Inode(Alloc)"}}
link_count{{"Inode(IncLink)"}}
old_dentry{{"Dentry(Start)"}}
old_dentry_clearino{{"Dentry(ClearIno)"}}

dram_alloc --> free_dentry --> pm_alloc --> alloc_dentry
alloc_dentry --> init --> init_dentry
link_count -.-> init
inode_alloc -.-> init
free_dentry --> alloc_bkptr --> alloc_bkptr_dentry
alloc_bkptr_dentry --> init_bkptr --> init_bkptr_dentry
get_old_dentry --> old_dentry 
old_dentry -.-> alloc_bkptr
init_bkptr_dentry --> clear_bkptr --> clear_bkptr_dentry
old_dentry_clearino -.-> clear_bkptr
old_dentry -.-> old_dentry_clearino

classDef empty width:0px,height:0px;
classDef gray fill:#888,stroke:#333,stroke-width:2px;
class dram_alloc,get_old_dentry gray
```
Allocating a dentry in PM involves setting its name field and any other metadata fields EXCEPT for the ino field. This diagram does NOT include dependencies for allocating/initializing . and .. dentries; these are a special case that is handled during `DirPageHeader` setup.

Safety:
- It's always safe to allocate a dentry in an empty (completely zeroed out slot). As long as its `ino` field is zeroed out, it does not refer to anything and just causes a space leak. 
- The `ino` field cannot be set for a dentry in `Alloc` state unless there is a newly-allocated *or* newly link-incremented inode for it to refer to. Otherwise, we risk a dangling pointer or a too-low link count, both of which are illegal inconsistencies.
- It *is* safe to set the `ino` field for a dentry in `SetBackpointer` state because the inode number comes from the old dentry (currently persistent in `Start` state), so the pointer won't dangle. Also, we've already set the backpointer to the old dentry, so we can roll back to the old name if necessary.
- It is not safe to clear the backpointer from a dentry in `InitBackpointer` state until the old dentry is in `ClearIno` state, since doing so would leave us unable to determine which two dentries were involved in the `rename` operation.
  - However, note that it is NOT safe to deallocate the old dentry until the new dentry is in `ClearBackpointer` state. If we were to free the old dentry before deleting the backpointer, we could break rule 2 of soft updates (nullify all pointers to a resource before reusing it) if the old dentry were re-allocated after a crash while the backpointer is still present in the new dentry.

**Dentry modification and deletion**
```mermaid
graph TD

get_dentry([Get dentry])
get_old_dentry([Find old dentry])
start_dentry["Dentry(Start)"]
zero_ino([Zero ino])
clearino_dentry["Dentry(ClearIno)"]
dealloc([Deallocate dentry])
dealloc_dentry["Dentry(Dealloc)"]
set_bkptr([Set backpointer])
set_bkptr_dentry["Dentry(SetBackpointer)"]
init_bkptr([Set ino])
init_bkptr_dentry["Dentry(InitBackpointer)"]
clear_bkptr([Clear backpointer])
clear_bkptr_dentry["Dentry(ClearBackpointer)"]

old_dentry{{"Dentry(Start)"}}
old_dentry_clearino{{"Dentry(ClearIno)"}}

get_dentry --> start_dentry
start_dentry --> zero_ino
zero_ino --> clearino_dentry
clearino_dentry --> dealloc
dealloc --> dealloc_dentry

start_dentry --> set_bkptr --> set_bkptr_dentry
get_old_dentry --> old_dentry
old_dentry -.-> set_bkptr
set_bkptr_dentry --> init_bkptr --> init_bkptr_dentry
init_bkptr_dentry --> clear_bkptr --> clear_bkptr_dentry
old_dentry_clearino -.-> clear_bkptr
old_dentry -.-> old_dentry_clearino

classDef empty width:0px,height:0px;
classDef gray fill:#888,stroke:#333,stroke-width:2px;
class get_dentry,get_old_dentry gray
```
Dentries are only modified during `rename`; the only other time they are updated is to deallocate them in `unlink` or `rmdir`. 

Safety:
- It's always safe to zero a dentry's inode field; this can result in a space leak as the dentry itself is still allocated, and the corresponding inode + pages will leak if this dentry is the last name for the file, but space leaks are safe.
- It's safe to set the backpointer field in a `Start` dentry because the remount scan will ignore the backpointer if the inodes in the new and old dentries don't match. 
- The backpointer can't be cleared until the old dentry's inode is zeroed, as described above.

**Directory page allocation**
```mermaid
graph TD

dram_alloc([Allocate page])
free_page["DirPageHeader(Free)"]
pm_alloc([Set page type on PM])
alloc_page["DirPageHeader(Alloc)"]
parent([Set .. in header])
parent_page["DirPageHeader(SetParent)"]
set_bkptr([Set . in header])
bkptr_page["DirPageHeader(Init)"]

new_inode{{"Inode(Alloc)"}}
start_inode{{"Inode(Start)"}}

dram_alloc --> free_page --> pm_alloc --> alloc_page
alloc_page --> parent --> parent_page
parent_page --> set_bkptr --> bkptr_page
new_inode & start_inode -.-> set_bkptr


classDef empty width:0px,height:0px;
classDef gray fill:#888,stroke:#333,stroke-width:2px;
class dram_alloc gray
```
To make things a bit easier on ourselves, each `DirPageHeader` includes the . and .. directory entries. This wastes a small amount of space (we don't really need the .. in each dentry, but it will be very small) but simplifies `mkdir` and some other operations. 

Safety:
- It is always safe to allocate a new page as this can only cause a space leak.
- The .. entry in the header will be ignored unless the . entry is also set and refers to a valid inode (i.e. one that is pointed to by at least one non-header dentry), so it's safe to set before the inode is fully initialized. 
- The . entry cannot be set until we have an allocated inode for it to point to. This ensures we can't break rule 2 of soft updates after a crash.

**Directory page deallocation**
```mermaid
graph TD

dram_alloc([Obtain dir page])
start_page["DirPageHeader(Start)"]
clear_ino([Clear . field])
clearino_page["DirPageHeader(ClearIno)"]
clearparent([Clear .. field])
clearparent_page["DirPageHeader(ClearParent)"]
dealloc([Deallocate page])
dealloc_page["DirPageHeader(Dealloc)"]

dram_alloc --> start_page
start_page --> clear_ino --> clearino_page
clearino_page --> clearparent --> clearparent_page
clearparent_page --> dealloc --> dealloc_page

classDef empty width:0px,height:0px;
classDef gray fill:#888,stroke:#333,stroke-width:2px;
class dram_alloc gray
```
`DirPageHeader`s are only modifed at allocation and deallocation. They are *not* modified and their typestate is not involved when dentries in the page they head are updated.

We may be able to combine the .. clearing and deallocation into a single step.

Safety:
- A directory page should only be deallocated when it contains no more allocated dentries. Since the `DirPageHeader` structure does not include or keep track of dentries, we'll need to keep track of this in DRAM. This property won't be enforceable at compile time.
- Since the .. dentry is ignored if the . dentry is empty, clearing the . dentry makes the page unreachable from any file.

TODO: data pages