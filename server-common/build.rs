fn main() {
    // A single `reuseport` cfg gate: the `reuseport` feature, on a Unix platform where
    // SO_REUSEPORT actually load-balances connections across a listener group. Apple platforms,
    // Solaris, illumos, and Cygwin accept the socket options but deliver every connection to one
    // listener, so a thread-per-core listener group offers nothing there.
    cfg_aliases::cfg_aliases! {
        reuseport: {
            all(
                feature = "reuseport",
                unix,
                not(target_os = "solaris"),
                not(target_os = "illumos"),
                not(target_os = "cygwin"),
                not(target_vendor = "apple")
            )
        },
    }
}
