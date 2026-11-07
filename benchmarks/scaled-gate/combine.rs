type CrcMatrix = [u32; 32];

const fn apply_crc_matrix(columns: &CrcMatrix, mut value: u32) -> u32 {
    let mut result = 0;
    while value != 0 {
        result ^= columns[value.trailing_zeros() as usize];
        value &= value - 1;
    }
    result
}

const fn zero_byte_powers() -> [CrcMatrix; 32] {
    let mut matrices = [[0; 32]; 32];
    let mut column = 0;
    while column < 32 {
        let mut crc = 1u32 << column;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
            bit += 1;
        }
        matrices[0][column] = crc;
        column += 1;
    }
    let mut power = 1;
    while power < 32 {
        let previous = matrices[power - 1];
        let mut column = 0;
        while column < 32 {
            matrices[power][column] = apply_crc_matrix(&previous, previous[column]);
            column += 1;
        }
        power += 1;
    }
    matrices
}
static ZERO_BYTE_POWERS: [CrcMatrix; 32] = zero_byte_powers();

fn combine_crc(mut prefix: u32, suffix: u32, mut suffix_bytes: u32) -> u32 {
    while suffix_bytes != 0 {
        let power = suffix_bytes.trailing_zeros() as usize;
        prefix = apply_crc_matrix(&ZERO_BYTE_POWERS[power], prefix);
        suffix_bytes &= suffix_bytes - 1;
    }
    prefix ^ suffix
}
