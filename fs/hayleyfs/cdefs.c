#include <linux/bitops.h>
#include <linux/cred.h>

unsigned long find_next_zero_bit_le_helper(const unsigned long *addr, unsigned long size, unsigned long offset) {
    return find_next_zero_bit_le(addr, size, offset);
}

int test_and_set_bit_le_helper(int nr, void* addr) {
   return test_and_set_bit(nr, addr);
}

void set_bit_helper(long nr, void* addr) {
    return set_bit(nr, addr);
}

int test_and_clear_bit_le_helper(int nr, void* addr) {
    return test_and_clear_bit_le(nr, addr);
}
