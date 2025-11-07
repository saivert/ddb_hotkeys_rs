macro_rules! debug {
    ($s:expr) => {
        {
        #[cfg(debug_assertions)]
        eprintln!($s)
        }
    };
}
