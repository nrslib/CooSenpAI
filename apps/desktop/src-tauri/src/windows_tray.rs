use tauri::image::Image;

pub(super) fn recording_icon() -> Image<'static> {
    const SIZE: u32 = 36;
    let mut pixels = vec![0_u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f64 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance =
                ((f64::from(x) - center).powi(2) + (f64::from(y) - center).powi(2)).sqrt();
            if distance <= 11.5 {
                let offset = ((y * SIZE + x) * 4) as usize;
                pixels[offset..offset + 4].copy_from_slice(&[224, 70, 70, 255]);
            }
        }
    }
    Image::new_owned(pixels, SIZE, SIZE)
}
