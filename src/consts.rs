use tokio::time::Duration;

pub const MAX_PACKET_LEN: usize = 256 * 1024;
pub const MAX_STRING_LEN: usize = 32 * 1024;
pub const HANDSHAKE_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub const DEFAULT_BYTES_PER_SEC: usize = 64 * 1024;
pub const DEFAULT_BURST_BYTES: usize = 128 * 1024;