use std::fmt::Display;

pub fn noun<N: PartialEq + From<u8>>(count: N, one: &str) -> String {
    if count == N::from(1) {
        one.to_string()
    } else {
        format!("{one}s")
    }
}

pub fn counted<N: PartialEq + From<u8> + Display>(count: N, one: &str) -> String {
    let number = count.to_string();
    format!("{number} {}", noun(count, one))
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

    #[test]
    fn the_noun_alone_is_singular_only_for_a_count_of_one() {
        assert_eq!(noun(1_usize, "node"), "node");
        assert_eq!(noun(2_usize, "node"), "nodes");
        assert_eq!(noun(0_u64, "node"), "nodes");
    }
}
