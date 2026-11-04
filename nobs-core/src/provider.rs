use std::ffi::OsString;

use crate::compose::Compose;
use crate::containerlab::Containerlab;

pub trait Provider {
    fn program(&self) -> &'static str;
    fn show_args(&self) -> Vec<OsString>;
}

impl Provider for Compose {
    fn program(&self) -> &'static str {
        Compose::PROGRAM
    }

    fn show_args(&self) -> Vec<OsString> {
        self.ps_args(&[])
    }
}

impl Provider for Containerlab {
    fn program(&self) -> &'static str {
        Containerlab::program(self)
    }

    fn show_args(&self) -> Vec<OsString> {
        self.inspect_args()
    }
}
