#[cfg(all(test, feature = "integration"))]
mod integration;

#[cfg(test)]
mod cluster_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod dispatcher_tests;
#[cfg(test)]
mod error_tests;
#[cfg(test)]
mod heartbeat_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod matching_tests;
#[cfg(test)]
mod proto_tests;
#[cfg(test)]
mod retry_tests;
#[cfg(test)]
mod sdk_tests;
