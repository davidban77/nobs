use std::fmt::Display;

pub fn counted<N: PartialEq + From<u8> + Display>(count: N, one: &str) -> String {
    if count == N::from(1) {
        format!("{count} {one}")
    } else {
        format!("{count} {one}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_of_one_takes_the_singular_noun() {
        assert_eq!(counted(1_usize, "service"), "1 service");
        assert_eq!(counted(1_u64, "node"), "1 node");
        assert_eq!(counted(2_usize, "service"), "2 services");
        assert_eq!(counted(0_u64, "node"), "0 nodes");
    }
}
