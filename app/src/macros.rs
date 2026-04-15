/// Create a `&'static mut` reference to a value using `static_cell`.
/// Used for embassy-net stack and TCP socket buffers that require
/// `'static` lifetime.
#[macro_export]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}
