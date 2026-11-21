// What rustc expanded for `probe_uint.trace`: this file after `uint_impl.rs`,
// the invocation copied from library/core/src/num/mod.rs lines 447-468.

struct U8;

impl U8 {
    uint_impl! {
        Self = u8,
        ActualT = u8,
        SignedT = i8,
        BITS = 8,
        BITS_MINUS_ONE = 7,
        MAX = 255,
        rot = 2,
        rot_op = "0x82",
        rot_result = "0xa",
        fsh_op = "0x36",
        fshl_result = "0x8",
        fshr_result = "0x8d",
        swap_op = "0x12",
        swapped = "0x12",
        reversed = "0x48",
        le_bytes = "[0x12]",
        be_bytes = "[0x12]",
        to_xe_bytes_doc = u8_xe_bytes_doc!(),
        from_xe_bytes_doc = u8_xe_bytes_doc!(),
        bound_condition = "",
    }
}

fn main() {}
