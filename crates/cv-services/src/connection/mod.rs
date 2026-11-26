//! The provider a page gets for a connection: built and swapped when the connection changes
//! ([`providers`]), watched for the 401 and 403 answers the access banner turns into actions
//! ([`access_issues`]), and counted per connection for Settings → Network activity and the
//! rate-limit banner ([`traffic`]).

pub mod access_issues;
pub mod providers;
pub mod traffic;
