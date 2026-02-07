pub mod valka {
    pub mod v1 {
        tonic::include_proto!("valka.v1");

        pub const FILE_DESCRIPTOR_SET: &[u8] =
            tonic::include_file_descriptor_set!("valka_descriptor");
    }
}

pub use valka::v1::*;
