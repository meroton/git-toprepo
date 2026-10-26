#[derive(Debug, Clone)]
pub(super) enum GitPacket<T> {
    Data(T),
    Flush,
    EndOfFile,
}
