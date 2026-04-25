// https://docs.espressif.com/projects/rust/esp-hal/1.0.0/esp32s3/esp_hal/lcd_cam/cam/index.html
// https://github.com/esp-rs/esp-hal/blob/main/qa-test/src/bin/lcd_cam_ov2640.rs

use esp_hal::{
    delay::Delay,
    i2c::master::{Config, I2c},
    lcd_cam::{
        LcdCam,
        cam::{self, Camera, Config as CamConfig, ConfigError},
    },
    peripherals::{DMA_CH0, I2C0, LCD_CAM},
    peripherals::{
        GPIO4, GPIO5, GPIO6, GPIO7, GPIO8, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13, GPIO15, GPIO16,
        GPIO17, GPIO18,
    },
    time::Rate,
};
use esp_println::println;

use crate::camera::camera_config::*;

pub fn cam_init<'a>(
    lcd_cam: LCD_CAM<'a>,
    dma_channel: DMA_CH0<'a>,
    i2c0: I2C0<'a>,
    cam_siod: GPIO4<'a>,
    cam_sioc: GPIO5<'a>,
    cam_xclk: GPIO15<'a>,
    cam_vsync: GPIO6<'a>,
    cam_href: GPIO7<'a>,
    cam_pclk: GPIO13<'a>,
    gpio11: GPIO11<'a>,
    gpio9: GPIO9<'a>,
    gpio8: GPIO8<'a>,
    gpio10: GPIO10<'a>,
    gpio12: GPIO12<'a>,
    gpio18: GPIO18<'a>,
    gpio17: GPIO17<'a>,
    gpio16: GPIO16<'a>,
) -> Result<Camera<'a>, ConfigError> {
    let cam_config = CamConfig::default().with_frequency(Rate::from_mhz(10));

    let lcd_cam = LcdCam::new(lcd_cam);
    let camera: Camera<'_> = Camera::new(lcd_cam.cam, dma_channel, cam_config)
        .unwrap()
        .with_master_clock(cam_xclk)
        .with_pixel_clock(cam_pclk)
        .with_vsync(cam_vsync)
        .with_h_enable(cam_href)
        .with_data0(gpio11)
        .with_data1(gpio9)
        .with_data2(gpio8)
        .with_data3(gpio10)
        .with_data4(gpio12)
        .with_data5(gpio18)
        .with_data6(gpio17)
        .with_data7(gpio16);

    let delay = Delay::new();

    delay.delay_millis(500u32);

    let i2c = I2c::new(i2c0, Config::default())
        .unwrap()
        .with_sda(cam_siod)
        .with_scl(cam_sioc);

    let mut sccb = Sccb::new(i2c);

    // Checking camera slv_address
    sccb.probe(OV3660_ADDRESS).unwrap();
    println!("Probe successful!");

    let pid = sccb.read(OV3660_ADDRESS, &[0x30, 0x0A]).unwrap();
    println!("Found PID of {:#02X}, and was expecting 0x36", pid);

    let pid = sccb.read(OV3660_ADDRESS, &[0x30, 0x0B]).unwrap();
    println!("Found PID of {:#02X}, and was expecting 0x60", pid);

    for (reg, value) in RESET_BLOCK {
        sccb.write(
            OV3660_ADDRESS,
            &[(reg >> 8) as u8, (reg & 0xff) as u8],
            *value,
        )
        .unwrap();
    }

    delay.delay_millis(20u32);

    for (reg, value) in INIT_BLOCK {
        sccb.write(
            OV3660_ADDRESS,
            &[(reg >> 8) as u8, (reg & 0xff) as u8],
            *value,
        )
        .unwrap();
    }

    for (reg, value) in SENSOR_FMT_JPEG {
        sccb.write(
            OV3660_ADDRESS,
            &[(reg >> 8) as u8, (reg & 0xff) as u8],
            *value,
        )
        .unwrap();
    }

    for (reg, value) in SENSOR_FRAMESIZE_SVGA {
        sccb.write(
            OV3660_ADDRESS,
            &[(reg >> 8) as u8, (reg & 0xff) as u8],
            *value,
        )
        .unwrap();
    }

    delay.delay_millis(200u32);

    Ok(camera)
}
