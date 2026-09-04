use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub(super) async fn read_line_bounded<R>(
    reader: &mut R,
    output: &mut Vec<u8>,
    limit: usize,
) -> io::Result<usize>
where
    R: AsyncBufRead + Unpin,
{
    let mut total = 0usize;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(total);
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if total.saturating_add(take) > limit {
            return Err(io::Error::other(
                "provider bridge response line exceeds limit",
            ));
        }
        output.extend_from_slice(&available[..take]);
        reader.consume(take);
        total += take;
        if output.last() == Some(&b'\n') {
            return Ok(total);
        }
    }
}

