#![allow(clippy::identity_op)]

extern crate x264_sys as ffi;

use ffi::x264::*;
use std::ffi::CString;
use std::mem;
use std::os::raw::c_int;
use std::ptr::null;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum X264Error {
    #[error("{0}")]
    GenericError(String),
}

pub struct Picture {
    pub pic: x264_picture_t,
    plane_size: [usize; 3],
    native: bool,
}

struct ColorspaceScale {
    w: [usize; 3],
    h: [usize; 3],
}
fn scale_from_csp(csp: u32) -> ColorspaceScale {
    match csp {
        X264_CSP_I420 => ColorspaceScale {
            w: [256 * 1, 256 / 2, 256 / 2],
            h: [256 * 1, 256 / 2, 256 / 2],
        },
        X264_CSP_YV12 => ColorspaceScale {
            w: [256 * 1, 256 / 2, 256 / 2],
            h: [256 * 1, 256 / 2, 256 / 2],
        },
        X264_CSP_NV12 => ColorspaceScale {
            w: [256 * 1, 256 * 1, 0],
            h: [256 * 1, 256 / 2, 0],
        },
        X264_CSP_NV21 => ColorspaceScale {
            w: [256 * 1, 256 * 1, 0],
            h: [256 * 1, 256 / 2, 0],
        },
        X264_CSP_I422 => ColorspaceScale {
            w: [256 * 1, 256 / 2, 256 / 2],
            h: [256 * 1, 256 * 1, 256 * 1],
        },
        X264_CSP_YV16 => ColorspaceScale {
            w: [256 * 1, 256 / 2, 256 / 2],
            h: [256 * 1, 256 * 1, 256 * 1],
        },
        X264_CSP_NV16 => ColorspaceScale {
            w: [256 * 1, 256 * 1, 0],
            h: [256 * 1, 256 * 1, 0],
        },
        X264_CSP_I444 => ColorspaceScale {
            w: [256 * 1, 256 * 1, 256 * 1],
            h: [256 * 1, 256 * 1, 256 * 1],
        },
        X264_CSP_YV24 => ColorspaceScale {
            w: [256 * 1, 256 * 1, 256 * 1],
            h: [256 * 1, 256 * 1, 256 * 1],
        },
        X264_CSP_BGR => ColorspaceScale {
            w: [256 * 3, 0, 0],
            h: [256 * 1, 0, 0],
        },
        X264_CSP_BGRA => ColorspaceScale {
            w: [256 * 4, 0, 0],
            h: [256 * 1, 0, 0],
        },
        X264_CSP_RGB => ColorspaceScale {
            w: [256 * 3, 0, 0],
            h: [256 * 1, 0, 0],
        },
        _ => unimplemented!(),
    }
}

impl Picture {
    /*
        pub fn new() -> Picture {
            let mut pic = unsafe { mem::MaybeUninit::uninit().assume_init() };

            unsafe { x264_picture_init(&mut pic as *mut x264_picture_t) };

            Picture { pic: pic }
        }
    */
    pub fn from_param(param: &Param) -> Result<Picture, X264Error> {
        let mut pic: x264_picture_t = unsafe { mem::MaybeUninit::uninit().assume_init() };

        let ret = unsafe {
            x264_picture_alloc(
                &mut pic as *mut x264_picture_t,
                param.par.i_csp,
                param.par.i_width,
                param.par.i_height,
            )
        };
        if ret < 0 {
            Err(X264Error::GenericError("Allocation Failure".to_string()))
        } else {
            let scale = scale_from_csp(param.par.i_csp as u32 & X264_CSP_MASK as u32);
            let bytes = 1 + (param.par.i_csp as u32 & X264_CSP_HIGH_DEPTH as u32);
            let mut plane_size = [0; 3];

            for (i, size) in plane_size
                .iter_mut()
                .enumerate()
                .take(pic.img.i_plane as usize)
            {
                *size = param.par.i_width as usize * scale.w[i] / 256
                    * bytes as usize
                    * param.par.i_height as usize
                    * scale.h[i]
                    / 256;
            }

            Ok(Picture {
                pic,
                plane_size,
                native: true,
            })
        }
    }

    pub fn as_slice<'a>(&'a self, plane: usize) -> Result<&'a [u8], &'static str> {
        if plane > self.pic.img.i_plane as usize {
            Err("Invalid Argument")
        } else {
            let size = self.plane_size[plane];
            Ok(unsafe { std::slice::from_raw_parts(self.pic.img.plane[plane], size) })
        }
    }

    pub fn as_mut_slice<'a>(&'a mut self, plane: usize) -> Result<&'a mut [u8], &'static str> {
        if plane > self.pic.img.i_plane as usize {
            Err("Invalid Argument")
        } else {
            let size = self.plane_size[plane];
            Ok(unsafe { std::slice::from_raw_parts_mut(self.pic.img.plane[plane], size) })
        }
    }

    pub fn set_timestamp(&mut self, pts: i64) {
        self.pic.i_pts = pts;
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        if self.native {
            unsafe { x264_picture_clean(&mut self.pic as *mut x264_picture_t) };
        }
    }
}

// TODO: Provide a builder API instead?
pub struct Param {
    par: x264_param_t,
}

impl Default for Param {
    fn default() -> Self {
        Self::new()
    }
}

impl Param {
    pub fn new() -> Param {
        let mut par = unsafe { mem::MaybeUninit::uninit().assume_init() };

        unsafe {
            x264_param_default(&mut par as *mut x264_param_t);
        }

        Param { par }
    }
    pub fn default_preset(tune: &str, preset: &str) -> Result<Param, &'static str> {
        let mut par = unsafe { mem::MaybeUninit::uninit().assume_init() };
        let c_preset = CString::new(tune).unwrap();
        let c_tune = CString::new(preset).unwrap();
        match unsafe {
            x264_param_default_preset(
                &mut par as *mut x264_param_t,
                c_preset.as_ptr(),
                c_tune.as_ptr(),
            )
        } {
            -1 => Err("Invalid Argument"),
            0 => Ok(Param { par }),
            _ => Err("Unexpected"),
        }
    }
    pub fn apply_profile(mut self, profile: &str) -> Result<Param, &'static str> {
        let p = CString::new(profile).unwrap();
        match unsafe { x264_param_apply_profile(&mut self.par, p.as_ptr() as *const i8) } {
            -1 => Err("Invalid Argument"),
            0 => Ok(self),
            _ => Err("Unexpected"),
        }
    }
    pub fn param_parse(mut self, name: &str, value: &str) -> Result<Param, &'static str> {
        let n = CString::new(name).unwrap();
        let v = CString::new(value).unwrap();
        match unsafe {
            x264_param_parse(
                &mut self.par,
                n.as_ptr() as *const i8,
                v.as_ptr() as *const i8,
            )
        } {
            -1 => Err("Invalid Argument"),
            0 => Ok(self),
            _ => Err("Unexpected"),
        }
    }

    /// Set the dimensions for the input video.
    pub fn set_dimension(mut self, width: i32, height: i32) -> Param {
        self.par.i_height = height as c_int;
        self.par.i_width = width as c_int;

        self
    }

    /// Set the size of the dim parameter used for foveation.
    pub fn set_fovea(mut self, dim: i32) -> Param {
        self.par.dim = dim as c_int;

        self
    }

    pub fn set_min_keyint(mut self, min_keyint: i32) -> Param {
        self.par.i_keyint_max = min_keyint as c_int;
        self.par.i_keyint_min = min_keyint as c_int;

        self
    }

    /// Disable scenecuts.
    pub fn set_no_scenecut(mut self) -> Param {
        self.par.i_scenecut_threshold = 0;

        self
    }

    /// Use constant QP mode with the specified QP.
    pub fn set_qp(mut self, qp: i32) -> Param {
        self.par.rc.i_qp_constant = qp;
        self.par.rc.i_rc_method = 0;

        self
    }

    /// Sets the default parameters to match those of using x264's CLI for the
    /// 4k video clip.
    pub fn set_x264_defaults(mut self) -> Param {
        self.par.dim = 32;
        self.par.cpu = 1111039;
        self.par.i_threads = 12;
        self.par.i_lookahead_threads = 12;
        self.par.b_sliced_threads = 1;
        self.par.b_deterministic = 1;
        self.par.b_cpu_independent = 0;
        self.par.i_sync_lookahead = 0;
        self.par.i_csp = 2;
        self.par.i_bitdepth = 8;
        self.par.i_level_idc = 51;
        self.par.i_nal_hrd = 0;

        self.par.vui.i_sar_height = 0;
        self.par.vui.i_sar_width = 0;
        self.par.vui.i_overscan = 0;
        self.par.vui.i_vidformat = 5;
        self.par.vui.b_fullrange = 0;
        self.par.vui.i_colorprim = 2;
        self.par.vui.i_transfer = 2;
        self.par.vui.i_colmatrix = -1;
        self.par.vui.i_chroma_loc = 0;

        self.par.i_frame_reference = 1;
        self.par.i_dpb_size = 1;
        self.par.i_scenecut_threshold = 0;
        self.par.b_intra_refresh = 0;
        self.par.i_bframe = 0;
        self.par.i_bframe_adaptive = 0;
        self.par.i_bframe_bias = 0;
        self.par.i_bframe_pyramid = 0;
        self.par.b_open_gop = 0;
        self.par.b_bluray_compat = 0;
        self.par.i_avcintra_class = 0;
        self.par.i_avcintra_flavor = 0;
        self.par.b_deblocking_filter = 1;
        self.par.i_deblocking_filter_alphac0 = 0;
        self.par.i_deblocking_filter_beta = 0;
        self.par.b_cabac = 1;
        self.par.i_cabac_init_idc = 0;
        self.par.b_interlaced = 0;
        self.par.b_constrained_intra = 0;
        self.par.i_cqm_preset = 0;
        self.par.b_full_recon = 0;

        self.par.analyse.intra = 3;
        self.par.analyse.inter = 3;
        self.par.analyse.b_transform_8x8 = 1;
        self.par.analyse.i_weighted_pred = 1;
        self.par.analyse.b_weighted_bipred = 0;
        self.par.analyse.i_direct_mv_pred = 0;
        self.par.analyse.i_chroma_qp_offset = 0;
        self.par.analyse.i_me_method = 0;
        self.par.analyse.i_me_range = 16;
        self.par.analyse.i_mv_range = 512;
        self.par.analyse.i_mv_range_thread = -1;
        self.par.analyse.i_subpel_refine = 1;
        self.par.analyse.b_chroma_me = 1;
        self.par.analyse.b_mixed_references = 0;
        self.par.analyse.i_trellis = 0;
        self.par.analyse.b_fast_pskip = 1;
        self.par.analyse.b_dct_decimate = 1;
        self.par.analyse.i_noise_reduction = 0;
        self.par.analyse.f_psy_rd = 1.0;
        self.par.analyse.f_psy_trellis = 0.0;
        self.par.analyse.b_psy = 1;
        self.par.analyse.b_mb_info = 0;
        self.par.analyse.b_mb_info_update = 0;
        self.par.analyse.b_psnr = 0;
        self.par.analyse.b_ssim = 0;

        self.par.rc.i_rc_method = 0;
        self.par.rc.i_qp_constant = 24;
        self.par.rc.i_qp_min = 21;
        self.par.rc.i_qp_max = 69;
        self.par.rc.i_qp_step = 4;
        self.par.rc.i_bitrate = 0;
        self.par.rc.f_rf_constant = 23.0;
        self.par.rc.f_rf_constant_max = 0.0;
        self.par.rc.f_rate_tolerance = 1.0;
        self.par.rc.i_vbv_max_bitrate = 0;
        self.par.rc.i_vbv_buffer_size = 0;
        self.par.rc.f_vbv_buffer_init = 0.9;
        self.par.rc.f_ip_factor = 1.4;
        self.par.rc.f_pb_factor = 1.3;
        self.par.rc.b_filler = 0;
        self.par.rc.i_aq_mode = 1;
        self.par.rc.f_aq_strength = 1.0;
        self.par.rc.b_mb_tree = 0;
        self.par.rc.i_lookahead = 0;
        self.par.rc.b_stat_write = 0;
        self.par.rc.b_stat_read = 0;
        self.par.rc.f_qcompress = 0.6;
        self.par.rc.f_qblur = 0.5;
        self.par.rc.f_complexity_blur = 20.0;
        self.par.rc.i_zones = 0;

        self.par.i_frame_packing = -1;
        self.par.i_alternative_transfer = 2;
        self.par.b_aud = 0;
        self.par.b_repeat_headers = 1;
        self.par.b_annexb = 1;
        self.par.i_sps_id = 0;
        self.par.b_vfr_input = 0;
        self.par.b_pulldown = 0;
        self.par.i_fps_num = 24;
        self.par.i_fps_den = 1;
        self.par.i_timebase_num = 1;
        self.par.i_timebase_den = 24;
        self.par.b_tff = 1;
        self.par.b_pic_struct = 0;
        self.par.b_fake_interlaced = 0;
        self.par.b_stitchable = 0;
        self.par.b_opencl = 0;
        self.par.i_opencl_device = 0;
        self.par.i_slice_max_size = 0;
        self.par.i_slice_max_mbs = 0;
        self.par.i_slice_min_mbs = 0;
        self.par.i_slice_count = 12;
        self.par.i_slice_count_max = 0;

        self
    }
}

// TODO: Expose a NAL abstraction
pub struct NalData {
    vec: Vec<u8>,
}

impl NalData {
    /*
     * x264 functions return x264_nal_t arrays that are valid only until another
     * function of that kind is called.
     *
     * Always copy the data over.
     *
     * TODO: Consider using Bytes as backing store.
     */
    fn from_nals(c_nals: *mut x264_nal_t, nb_nal: usize) -> NalData {
        let mut data = NalData { vec: Vec::new() };

        for i in 0..nb_nal {
            let nal = unsafe { Box::from_raw(c_nals.add(i)) };

            let payload =
                unsafe { std::slice::from_raw_parts(nal.p_payload, nal.i_payload as usize) };

            data.vec.extend_from_slice(payload);

            // mem::forget(payload);
            mem::forget(nal);
        }

        data
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.vec.as_slice()
    }
}

pub struct Encoder {
    enc: *mut x264_t,
}

impl Encoder {
    pub fn open(par: &mut Param) -> Result<Encoder, &'static str> {
        let enc = unsafe { x264_encoder_open(&mut par.par as *mut x264_param_t) };

        if enc.is_null() {
            Err("Out of Memory")
        } else {
            Ok(Encoder { enc })
        }
    }

    pub fn get_headers(&mut self) -> Result<NalData, &'static str> {
        let mut nb_nal: c_int = 0;
        let mut c_nals: *mut x264_nal_t = unsafe { mem::MaybeUninit::uninit().assume_init() };

        let bytes = unsafe {
            x264_encoder_headers(
                self.enc,
                &mut c_nals as *mut *mut x264_nal_t,
                &mut nb_nal as *mut c_int,
            )
        };

        if bytes < 0 {
            Err("Encoding Headers Failed")
        } else {
            Ok(NalData::from_nals(c_nals, nb_nal as usize))
        }
    }

    pub fn encode<'a, P>(&mut self, pic: P) -> Result<Option<(NalData, i64, i64)>, &'static str>
    where
        P: Into<Option<&'a Picture>>,
    {
        let mut pic_out: x264_picture_t = unsafe { mem::MaybeUninit::uninit().assume_init() };
        let mut c_nals: *mut x264_nal_t = unsafe { mem::MaybeUninit::uninit().assume_init() };
        let mut nb_nal: c_int = 0;
        let c_pic = pic
            .into()
            .map_or_else(null, |v| &v.pic as *const x264_picture_t);

        let ret = unsafe {
            x264_encoder_encode(
                self.enc,
                &mut c_nals as *mut *mut x264_nal_t,
                &mut nb_nal as *mut c_int,
                c_pic as *mut x264_picture_t,
                &mut pic_out as *mut x264_picture_t,
            )
        };
        if ret < 0 {
            Err("Error encoding")
        } else if nb_nal > 0 {
            let data = NalData::from_nals(c_nals, nb_nal as usize);
            Ok(Some((data, pic_out.i_pts, pic_out.i_dts)))
        } else {
            Ok(None)
        }
    }

    pub fn delayed_frames(&self) -> bool {
        unsafe { x264_encoder_delayed_frames(self.enc) != 0 }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { x264_encoder_close(self.enc) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open() {
        let mut par = Param::new().set_dimension(640, 480);

        let mut enc = Encoder::open(&mut par).unwrap();

        let headers = enc.get_headers().unwrap();

        println!("Headers len {}", headers.as_bytes().len());
    }

    #[test]
    fn test_picture() {
        let par = Param::new().set_dimension(640, 480);
        {
            let mut pic = Picture::from_param(&par).unwrap();
            {
                let p = pic.as_mut_slice(0).unwrap();
                p[0] = 1;
            }
            let p = pic.as_slice(0).unwrap();

            assert_eq!(p[0], 1);
        }
    }

    #[test]
    fn test_encode() {
        let mut par = Param::new().set_dimension(640, 480);
        let mut enc = Encoder::open(&mut par).unwrap();
        let mut pic = Picture::from_param(&par).unwrap();

        let headers = enc.get_headers().unwrap();

        println!("Headers len {}", headers.as_bytes().len());

        for pts in 0..5 {
            pic.set_timestamp(pts as i64);
            let ret = enc.encode(&pic).unwrap();
            match ret {
                Some((_, pts, dts)) => println!("Frame pts {}, dts {}", pts, dts),
                _ => (),
            }
        }

        while enc.delayed_frames() {
            let ret = enc.encode(None).unwrap();
            match ret {
                Some((_, pts, dts)) => println!("Frame pts {}, dts {}", pts, dts),
                _ => (),
            }
        }
    }
}
