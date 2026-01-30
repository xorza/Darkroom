use super::*;

#[test]
fn test_par_chunks_mut_auto_offsets() {
    let mut data: Vec<usize> = vec![0; 100];
    data.par_chunks_mut_auto().for_each(|(offset, chunk)| {
        for (i, val) in chunk.iter_mut().enumerate() {
            *val = offset + i;
        }
    });
    for (i, &v) in data.iter().enumerate() {
        assert_eq!(v, i);
    }
}

#[test]
fn test_par_rows_mut_auto_offsets() {
    let width = 10;
    let height = 20;
    let mut data: Vec<usize> = vec![0; width * height];

    data.par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, chunk)| {
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
fn test_par_rows_mut_auto_row_alignment() {
    let width = 7;
    let height = 13;
    let mut data: Vec<u32> = vec![0; width * height];

    data.par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, chunk)| {
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
fn test_par_zip_two_slices() {
    let width = 8;
    let height = 10;
    let mut a: Vec<f32> = vec![0.0; width * height];
    let mut b: Vec<f32> = vec![0.0; width * height];

    a.as_mut_slice()
        .par_zip(&mut b)
        .par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, (a_chunk, b_chunk))| {
            let rows_in_chunk = a_chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = chunk_start_row + local_y;
                for x in 0..width {
                    let idx = local_y * width + x;
                    a_chunk[idx] = (y * width + x) as f32;
                    b_chunk[idx] = (y * width + x) as f32 * 2.0;
                }
            }
        });

    for i in 0..width * height {
        assert_eq!(a[i], i as f32);
        assert_eq!(b[i], i as f32 * 2.0);
    }
}

#[test]
fn test_par_zip_three_slices() {
    let width = 5;
    let height = 8;
    let mut a: Vec<i32> = vec![0; width * height];
    let mut b: Vec<i32> = vec![0; width * height];
    let mut c: Vec<i32> = vec![0; width * height];

    a.as_mut_slice()
        .par_zip(&mut b)
        .par_zip(&mut c)
        .par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, (a_chunk, b_chunk, c_chunk))| {
            let rows_in_chunk = a_chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = chunk_start_row + local_y;
                for x in 0..width {
                    let idx = local_y * width + x;
                    let val = (y * width + x) as i32;
                    a_chunk[idx] = val;
                    b_chunk[idx] = val * 2;
                    c_chunk[idx] = val * 3;
                }
            }
        });

    for i in 0..width * height {
        assert_eq!(a[i], i as i32);
        assert_eq!(b[i], i as i32 * 2);
        assert_eq!(c[i], i as i32 * 3);
    }
}

#[test]
fn test_par_zip_four_slices() {
    let width = 4;
    let height = 6;
    let mut a: Vec<u8> = vec![0; width * height];
    let mut b: Vec<u8> = vec![0; width * height];
    let mut c: Vec<u8> = vec![0; width * height];
    let mut d: Vec<u8> = vec![0; width * height];

    a.as_mut_slice()
        .par_zip(&mut b)
        .par_zip(&mut c)
        .par_zip(&mut d)
        .par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, (a_chunk, b_chunk, c_chunk, d_chunk))| {
            let rows_in_chunk = a_chunk.len() / width;
            for local_y in 0..rows_in_chunk {
                let y = chunk_start_row + local_y;
                for x in 0..width {
                    let idx = local_y * width + x;
                    let val = (y * width + x) as u8;
                    a_chunk[idx] = val;
                    b_chunk[idx] = val.wrapping_add(1);
                    c_chunk[idx] = val.wrapping_add(2);
                    d_chunk[idx] = val.wrapping_add(3);
                }
            }
        });

    for i in 0..width * height {
        let val = i as u8;
        assert_eq!(a[i], val);
        assert_eq!(b[i], val.wrapping_add(1));
        assert_eq!(c[i], val.wrapping_add(2));
        assert_eq!(d[i], val.wrapping_add(3));
    }
}

#[test]
#[should_panic(expected = "equal length")]
fn test_par_zip_unequal_lengths_panics() {
    let mut a: Vec<f32> = vec![0.0; 100];
    let mut b: Vec<f32> = vec![0.0; 50];

    a.as_mut_slice()
        .par_zip(&mut b)
        .par_rows_mut_auto(10)
        .for_each(|_| {});
}

#[test]
fn test_par_rows_mut_auto_single_row() {
    let width = 100;
    let height = 1;
    let mut data: Vec<usize> = vec![0; width * height];

    data.par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, chunk)| {
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
fn test_par_rows_mut_auto_large_image() {
    let width = 1920;
    let height = 1080;
    let mut data: Vec<u32> = vec![0; width * height];

    data.par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, chunk)| {
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

    let iter = data.par_rows_mut_auto(width);
    assert!(iter.len() > 0);
    assert!(iter.len() <= height);
}

#[test]
fn test_par_zip_different_types() {
    let width = 6;
    let height = 4;
    let mut floats: Vec<f32> = vec![0.0; width * height];
    let mut ints: Vec<i32> = vec![0; width * height];

    floats
        .as_mut_slice()
        .par_zip(&mut ints)
        .par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, (f_chunk, i_chunk))| {
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
        });

    for i in 0..width * height {
        assert_eq!(floats[i], i as f32 * 0.5);
        assert_eq!(ints[i], i as i32 * 2);
    }
}
