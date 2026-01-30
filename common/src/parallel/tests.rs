use super::*;

#[test]
fn test_par_chunks_auto_offsets() {
    let mut data: Vec<usize> = vec![0; 100];
    par_chunks_auto(&mut data).for_each(|(offset, chunk)| {
        for (i, val) in chunk.iter_mut().enumerate() {
            *val = offset + i;
        }
    });
    for (i, &v) in data.iter().enumerate() {
        assert_eq!(v, i);
    }
}

#[test]
fn test_par_chunks_auto_aligned_offsets() {
    let width = 10;
    let height = 20;
    let mut data: Vec<usize> = vec![0; width * height];

    par_chunks_auto_aligned(&mut data, width).for_each(|(chunk_start_row, chunk)| {
        let rows_in_chunk = chunk.len() / width;
        for local_y in 0..rows_in_chunk {
            let y = chunk_start_row + local_y;
            for x in 0..width {
                chunk[local_y * width + x] = y * width + x;
            }
        }
    });

    for (i, &v) in data.iter().enumerate() {
        assert_eq!(v, i);
    }
}

#[test]
fn test_par_chunks_auto_aligned_row_alignment() {
    let width = 7;
    let height = 13;
    let mut data: Vec<u32> = vec![0; width * height];

    par_chunks_auto_aligned(&mut data, width).for_each(|(chunk_start_row, chunk)| {
        assert_eq!(chunk.len() % width, 0, "Chunk not row-aligned");
        let rows_in_chunk = chunk.len() / width;
        for local_y in 0..rows_in_chunk {
            let y = chunk_start_row + local_y;
            for x in 0..width {
                chunk[local_y * width + x] = y as u32;
            }
        }
    });

    for y in 0..height {
        for x in 0..width {
            assert_eq!(data[y * width + x], y as u32);
        }
    }
}

#[test]
fn test_par_chunks_auto_aligned_zip2() {
    let width = 8;
    let height = 10;
    let mut a: Vec<f32> = vec![0.0; width * height];
    let mut b: Vec<f32> = vec![0.0; width * height];

    par_chunks_auto_aligned_zip2(&mut a, &mut b, width).for_each(
        |(chunk_start_row, (a_chunk, b_chunk))| {
            let rows_in_chunk = a_chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = chunk_start_row + local_y;
                for x in 0..width {
                    let idx = local_y * width + x;
                    a_chunk[idx] = (y * width + x) as f32;
                    b_chunk[idx] = (y * width + x) as f32 * 2.0;
                }
            }
        },
    );

    for i in 0..width * height {
        assert_eq!(a[i], i as f32);
        assert_eq!(b[i], i as f32 * 2.0);
    }
}

#[test]
#[should_panic(expected = "equal length")]
fn test_par_chunks_auto_aligned_zip2_unequal_lengths_panics() {
    let mut a: Vec<f32> = vec![0.0; 100];
    let mut b: Vec<f32> = vec![0.0; 50];

    par_chunks_auto_aligned_zip2(&mut a, &mut b, 10).for_each(|_| {});
}

#[test]
fn test_par_chunks_auto_aligned_single_row() {
    let width = 100;
    let height = 1;
    let mut data: Vec<usize> = vec![0; width * height];

    par_chunks_auto_aligned(&mut data, width).for_each(|(chunk_start_row, chunk)| {
        assert_eq!(chunk_start_row, 0);
        for (x, val) in chunk.iter_mut().enumerate() {
            *val = x;
        }
    });

    for (i, &v) in data.iter().enumerate() {
        assert_eq!(v, i);
    }
}

#[test]
fn test_par_chunks_auto_aligned_large_image() {
    let width = 1920;
    let height = 1080;
    let mut data: Vec<u32> = vec![0; width * height];

    par_chunks_auto_aligned(&mut data, width).for_each(|(chunk_start_row, chunk)| {
        let rows_in_chunk = chunk.len() / width;
        for local_y in 0..rows_in_chunk {
            let y = chunk_start_row + local_y;
            for x in 0..width {
                chunk[local_y * width + x] = (y * width + x) as u32;
            }
        }
    });

    // Spot check some values
    assert_eq!(data[0], 0);
    assert_eq!(data[width], width as u32);
    assert_eq!(data[width * height - 1], (width * height - 1) as u32);
}

#[test]
fn test_indexed_parallel_iterator_len() {
    let width = 10;
    let height = 100;
    let mut data: Vec<f32> = vec![0.0; width * height];

    let iter = par_chunks_auto_aligned(&mut data, width);
    assert!(iter.len() > 0);
    assert!(iter.len() <= height);
}

#[test]
fn test_par_chunks_auto_aligned_zip2_different_types() {
    let width = 6;
    let height = 4;
    let mut floats: Vec<f32> = vec![0.0; width * height];
    let mut ints: Vec<i32> = vec![0; width * height];

    par_chunks_auto_aligned_zip2(&mut floats, &mut ints, width).for_each(
        |(chunk_start_row, (f_chunk, i_chunk))| {
            let rows_in_chunk = f_chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = chunk_start_row + local_y;
                for x in 0..width {
                    let idx = local_y * width + x;
                    let val = y * width + x;
                    f_chunk[idx] = val as f32 * 0.5;
                    i_chunk[idx] = val as i32 * 2;
                }
            }
        },
    );

    for i in 0..width * height {
        assert_eq!(floats[i], i as f32 * 0.5);
        assert_eq!(ints[i], i as i32 * 2);
    }
}
