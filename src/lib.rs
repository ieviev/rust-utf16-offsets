// based on https://github.com/github/rust-gems/blob/main/crates/string-offsets/src/bitrank.rs
// this is a minimal wasm compatible utf-16 only version of the original code

type SubblockBits = u128;
// Static sizing of the various components of the data structure.
const BITS_PER_BLOCK: usize = 16384;
const BITS_PER_SUB_BLOCK: usize = SubblockBits::BITS as usize;
const SUB_BLOCKS_PER_BLOCK: usize = BITS_PER_BLOCK / BITS_PER_SUB_BLOCK;

#[derive(Clone, Debug)]
struct Block {
    rank: u64,
    sub_blocks: [u16; SUB_BLOCKS_PER_BLOCK],
    bits: [SubblockBits; SUB_BLOCKS_PER_BLOCK],
}

impl Block {
    fn set(&mut self, index: usize) {
        debug_assert!(index < BITS_PER_BLOCK);
        let chunk_idx = index / BITS_PER_SUB_BLOCK;
        let bit_idx = index % BITS_PER_SUB_BLOCK;
        let mask = 1 << ((BITS_PER_SUB_BLOCK - 1) - bit_idx);
        debug_assert_eq!(self.bits[chunk_idx] & mask, 0, "toggling bits off indicates that the original data was incorrect, most likely containing duplicate values.");
        self.bits[chunk_idx] ^= mask;
    }

    fn rank_select(&self, local_idx: usize) -> (usize, Option<usize>) {
        let mut rank = self.rank as usize;
        let sub_block = local_idx / BITS_PER_SUB_BLOCK;
        rank += self.sub_blocks[sub_block] as usize;

        let remainder = local_idx % BITS_PER_SUB_BLOCK;

        let last_chunk = local_idx / BITS_PER_SUB_BLOCK;
        let masked = if remainder == 0 {
            0
        } else {
            self.bits[last_chunk] >> (BITS_PER_SUB_BLOCK - remainder)
        };
        rank += masked.count_ones() as usize;
        let select = if masked == 0 {
            None
        } else {
            Some(local_idx - masked.trailing_zeros() as usize - 1)
        };
        (rank, select)
    }

    fn total_rank(&self) -> usize {
        self.sub_blocks[SUB_BLOCKS_PER_BLOCK - 1] as usize
            + self.rank as usize
            + self.bits[SUB_BLOCKS_PER_BLOCK - 1..]
                .iter()
                .map(|c| c.count_ones() as usize)
                .sum::<usize>()
    }
}

#[derive(Default)]
pub struct BitRankBuilder {
    blocks: Vec<Block>,
}

impl BitRankBuilder {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            blocks: Vec::with_capacity(cap.div_ceil(BITS_PER_BLOCK)),
        }
    }

    fn finish_last_block(&mut self) -> u64 {
        if let Some(block) = self.blocks.last_mut() {
            let mut local_rank = 0;
            for (i, chunk) in block.bits.iter().enumerate() {
                block.sub_blocks[i] = local_rank;
                local_rank += chunk.count_ones() as u16;
            }
            block.rank + local_rank as u64
        } else {
            0
        }
    }

    pub fn push(&mut self, position: usize) {
        let block_id = position / BITS_PER_BLOCK;
        debug_assert!(
            self.blocks.len() <= block_id + 1,
            "positions must be increasing!"
        );
        if block_id >= self.blocks.len() {
            let curr_rank = self.finish_last_block();
            while block_id >= self.blocks.len() {
                const ZERO_BLOCK: Block = Block {
                    rank: 0,
                    sub_blocks: [0; SUB_BLOCKS_PER_BLOCK],
                    bits: [0; SUB_BLOCKS_PER_BLOCK],
                };
                self.blocks.push(ZERO_BLOCK);
                self.blocks.last_mut().expect("just inserted").rank = curr_rank;
            }
        }
        self.blocks
            .last_mut()
            .expect("just ensured there are enough blocks")
            .set(position % BITS_PER_BLOCK);
    }

    pub fn finish(mut self) -> BitRank {
        self.finish_last_block();
        BitRank {
            blocks: self.blocks,
        }
    }
}

#[derive(Clone)]
pub struct BitRank {
    blocks: Vec<Block>,
}

impl BitRank {
    pub fn rank(&self, idx: usize) -> usize {
        self.rank_select(idx).0
    }

    pub fn max_rank(&self) -> usize {
        self.blocks
            .last()
            .map(|b| b.total_rank())
            .unwrap_or_default()
    }

    pub fn rank_select(&self, idx: usize) -> (usize, Option<usize>) {
        let block_num = idx / BITS_PER_BLOCK;
        if block_num >= self.blocks.len() {
            (self.max_rank(), None)
        } else {
            let (rank, b_idx) = self.blocks[block_num].rank_select(idx % BITS_PER_BLOCK);
            (rank, b_idx.map(|i| (block_num * BITS_PER_BLOCK) + i))
        }
    }
}

pub struct Offsets {
    utf8_to_utf16: BitRank,
}

impl Offsets {
    fn new_converter(content: &[u8]) -> Offsets {
        #[inline(always)]
        fn utf8_width(c: u8) -> usize {
            const UTF8_WIDTH: u64 = 0x4322_0000_1111_1111;
            ((UTF8_WIDTH >> ((c >> 4) * 4)) & 0xf) as usize
        }
        fn utf8_to_utf16_width(content: &[u8]) -> usize {
            let len = utf8_width(content[0]);
            match len {
                0 => 0,
                1..=3 => 1,
                4 => 2,
                _ => panic!("invalid utf8 char width: {}", len),
            }
        }
        let n = content.len();
        let mut utf16_builder = BitRankBuilder::with_capacity(n);
        let mut i = 0;
        while i < content.len() {
            let c = content[i];
            let utf8_len = utf8_width(c).max(1);
            if i > 0 {
                utf16_builder.push(i - 1);
            }
            if utf8_to_utf16_width(&content[i..]) > 1 {
                utf16_builder.push(i);
            }
            i += utf8_len;
        }
        if !content.is_empty() {
            utf16_builder.push(content.len() - 1);
        }

        Offsets {
            utf8_to_utf16: utf16_builder.finish(),
        }
    }
    pub fn new(content: &str) -> Self {
        Self::new_converter(content.as_bytes())
    }
    pub fn from_utf8_bytes(content: &[u8]) -> Self {
        Self::new_converter(content)
    }
    pub fn utf8_to_utf16(&self, byte_number: usize) -> usize {
        self.utf8_to_utf16.rank(byte_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utf16_2() {
        let input = r"email..email@example.com
あいうえお@example.com
❤️ line0 ❤️Á 👋 @ a
emailexample.com (Joe Smith)";
        let offsets = Offsets::new(input);
        assert_eq!(offsets.utf8_to_utf16(30), 26);
    }
}
