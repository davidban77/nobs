#!/bin/sh
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$(dirname "$0")/../../Cargo.toml")
if [ $# -eq 0 ]; then
    echo "$version"
elif [ "$1" = "v$version" ]; then
    echo "tag $1 is v$version, the version of Cargo.toml"
else
    echo "tag $1 is not v$version, the version of Cargo.toml" >&2
    exit 1
fi
