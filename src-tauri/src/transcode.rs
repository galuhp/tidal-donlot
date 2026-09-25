//! Konversi hasil unduhan (FLAC / ALAC / AAC) menjadi MP3.
//!
//! - **Decode**: `symphonia` (100% pure Rust) — FLAC, ALAC, AAC/MP4, MP3.
//! - **Encode**: LAME 3.100 lewat crate `mp3lame-encoder` (source LAME ikut
//!   dikompilasi saat build).
//!
//! Jadi tidak perlu memasang `ffmpeg` atau binary eksternal apa pun.

use std::fs::File;
use std::num::NonZeroU32;
use std::path::Path;

use lofty::prelude::*;
use lofty::probe::Probe;
use mp3lame_encoder::{
    Bitrate, Builder, Encoder, FlushNoGap, InterleavedPcm, MonoPcm, Quality, VbrMode,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Pilihan konversi MP3 di UI (dropdown "Format").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Mode {
    /// CBR: bitrate disamakan dengan bitrate file sumber (maksimal 320 kbps).
    SameBitrate,
    /// VBR LAME `-V0` (~245 kbps rata-rata, kualitas tertinggi).
    Vbr0,
}

/// Nilai `format` dari UI → mode konversi. `None` = simpan file apa adanya
/// (Lossless → FLAC, High/Low → AAC di dalam `.m4a`).
pub fn parse_mode(format: &str) -> Option<Mp3Mode> {
    match format.trim().to_ascii_lowercase().as_str() {
        "mp3" | "mp3_same" | "same" | "cbr" => Some(Mp3Mode::SameBitrate),
        "mp3_vbr0" | "vbr0" | "v0" => Some(Mp3Mode::Vbr0),
        _ => None,
    }
}

/// Bitrate CBR yang dikenal encoder MP3 (kbps), urut naik.
const MP3_KBPS: [u16; 16] = [
    8, 16, 24, 32, 40, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

/// Sample rate keluaran yang didukung LAME.
const LAME_RATES: [u32; 9] = [
    8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
];

/// Bitrate sumber (kbps) → nilai CBR MP3 terdekat (kbps).
///
/// Lossless biasanya ~800–1400 kbps (FLAC 16/44.1 = 1411 kbps), jadi otomatis
/// mentok ke 320 kbps — batas tertinggi MP3. AAC 320 tetap 320, AAC 96 tetap 96.
fn snap_kbps(source_kbps: Option<u32>) -> u16 {
    let target = source_kbps.unwrap_or(320).clamp(8, 320) as i32;
    MP3_KBPS
        .iter()
        .copied()
        .min_by_key(|kbps| ((*kbps as i32) - target).abs())
        .unwrap_or(320)
}

/// `u16` kbps → variant `Bitrate`.
fn bitrate_of(kbps: u16) -> Bitrate {
    match kbps {
        8 => Bitrate::Kbps8,
        16 => Bitrate::Kbps16,
        24 => Bitrate::Kbps24,
        32 => Bitrate::Kbps32,
        40 => Bitrate::Kbps40,
        48 => Bitrate::Kbps48,
        64 => Bitrate::Kbps64,
        80 => Bitrate::Kbps80,
        96 => Bitrate::Kbps96,
        112 => Bitrate::Kbps112,
        128 => Bitrate::Kbps128,
        160 => Bitrate::Kbps160,
        192 => Bitrate::Kbps192,
        224 => Bitrate::Kbps224,
        256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    }
}

/// Sample rate keluaran untuk LAME. Rate Hi-Res (88.2/96/176.4/192 kHz) tidak
/// didukung, jadi diturunkan ke rate "keluarga" terdekat yang habis membagi
/// (96k→48k, 88.2k→44.1k, 192k→48k). LAME sendiri yang resample karena
/// `in_samplerate != out_samplerate`.
fn lame_out_rate(rate: u32) -> Option<NonZeroU32> {
    if LAME_RATES.contains(&rate) {
        return NonZeroU32::new(rate);
    }
    if let Some(divisor) = LAME_RATES.iter().rev().copied().find(|r| rate % r == 0) {
        return NonZeroU32::new(divisor);
    }
    let nearest = LAME_RATES
        .iter()
        .rev()
        .copied()
        .find(|r| *r < rate)
        .unwrap_or(48_000);
    NonZeroU32::new(nearest)
}

/// Bitrate audio (kbps) file hasil unduhan — dipakai untuk mode "bitrate sama".
pub fn probe_bitrate_kbps(path: &Path) -> Option<u32> {
    Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .read()
        .ok()?
        .properties()
        .audio_bitrate()
}

/// Bikin encoder LAME sesuai rate/channel sumber & mode yang dipilih.
fn make_encoder(
    in_rate: u32,
    channels: u8,
    mode: Mp3Mode,
    source_kbps: Option<u32>,
) -> Result<Encoder, String> {
    let out_rate = lame_out_rate(in_rate).ok_or("sample rate tidak valid")?;
    let builder = Builder::new()
        .ok_or("gagal mengalokasikan encoder LAME")?
        .with_num_channels(channels)
        .map_err(|e| format!("jumlah channel tidak didukung: {e:?}"))?
        .with_sample_rate(in_rate)
        .map_err(|e| format!("sample rate {in_rate} Hz tidak didukung: {e:?}"))?
        .with_output_sample_rate(Some(out_rate))
        .map_err(|e| format!("output sample rate tidak didukung: {e:?}"))?;

    let builder = match mode {
        Mp3Mode::SameBitrate => builder
            .with_brate(bitrate_of(snap_kbps(source_kbps)))
            .map_err(|e| format!("bitrate tidak didukung: {e:?}"))?
            .with_vbr_mode(VbrMode::Off)
            .map_err(|e| format!("mode CBR gagal diset: {e:?}"))?,
        Mp3Mode::Vbr0 => builder
            .with_vbr_mode(VbrMode::Mtrh)
            .map_err(|e| format!("mode VBR gagal diset: {e:?}"))?
            .with_vbr_quality(Quality::Best)
            .map_err(|e| format!("VBR quality gagal diset: {e:?}"))?,
    };

    builder
        // q=1: kualitas hampir setara q=0 tapi jauh lebih cepat.
        .with_quality(Quality::SecondBest)
        .map_err(|e| format!("quality gagal diset: {e:?}"))?
        .build()
        .map_err(|e| format!("inisialisasi encoder LAME gagal: {e:?}"))
}

/// Konversi `input` (FLAC/ALAC/AAC/MP3) → MP3 di `output`.
///
/// Fungsi blocking — panggil lewat `spawn_blocking` supaya runtime async tidak
/// ikut tertahan selama encode berjalan.
pub fn convert_to_mp3(
    input: &Path,
    output: &Path,
    mode: Mp3Mode,
    source_kbps: Option<u32>,
) -> Result<(), String> {
    let file = File::open(input).map_err(|e| format!("buka {} gagal: {e}", input.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("format audio tidak dikenali: {e}"))?;
    let mut format = probed.format;

    // Pilih track audio: utamakan track yang punya sample rate & channel.
    let tracks = format.tracks();
    let track = tracks
        .iter()
        .find(|t| t.codec_params.sample_rate.is_some() && t.codec_params.channels.is_some())
        .or_else(|| tracks.iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL))
        .ok_or_else(|| "file tidak berisi track audio".to_string())?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| format!("codec tidak didukung: {e}"))?;

    let mut encoder: Option<Encoder> = None;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut channels: usize = 0;
    let mut mp3: Vec<u8> = Vec::new();
    // Buffer sementara untuk sumber >2 kanal (dipakai 2 kanal pertama).
    let mut stereo: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // Akhir stream: demuxer membalas EOF sebagai IoError.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("gagal membaca audio: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // Frame rusak: lewati, sisa lagu tetap bisa dikonversi.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decode audio gagal: {e}")),
        };

        let spec = *decoded.spec();
        if spec.rate == 0 {
            return Err("sample rate audio tidak valid".into());
        }
        if encoder.is_none() {
            let source_channels = spec.channels.count();
            if source_channels == 0 {
                return Err("file tidak punya channel audio".into());
            }
            channels = source_channels.min(2);
            encoder = Some(make_encoder(spec.rate, channels as u8, mode, source_kbps)?);
        }
        let encoder = encoder.as_mut().ok_or("encoder belum siap")?;

        // symphonia menormalkan semua format ke f32 interleaved.
        let buf = sample_buf
            .get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        buf.copy_interleaved_ref(decoded);

        let samples = buf.samples();
        if channels == 1 {
            if samples.is_empty() {
                continue;
            }
            mp3.reserve(mp3lame_encoder::max_required_buffer_size(samples.len()));
            encoder
                .encode_to_vec(MonoPcm(samples), &mut mp3)
                .map_err(|e| format!("encode MP3 gagal: {e:?}"))?;
        } else if channels == 2 {
            if samples.len() < 2 {
                continue;
            }
            mp3.reserve(mp3lame_encoder::max_required_buffer_size(samples.len() / 2));
            encoder
                .encode_to_vec(InterleavedPcm(samples), &mut mp3)
                .map_err(|e| format!("encode MP3 gagal: {e:?}"))?;
        } else {
            // Sumber >2 kanal (mis. 5.1): ambil 2 kanal pertama.
            stereo.clear();
            stereo.extend(
                samples
                    .chunks_exact(channels)
                    .flat_map(|frame| [frame[0], frame[1]]),
            );
            if stereo.len() < 2 {
                continue;
            }
            mp3.reserve(mp3lame_encoder::max_required_buffer_size(stereo.len() / 2));
            encoder
                .encode_to_vec(InterleavedPcm(&stereo), &mut mp3)
                .map_err(|e| format!("encode MP3 gagal: {e:?}"))?;
        }
    }

    let mut encoder = encoder.ok_or_else(|| "tidak ada audio yang berhasil didecode".to_string())?;

    // Flush sisa frame (padding) supaya tidak ada sampel yang hilang.
    mp3.reserve(7_200);
    encoder
        .flush_to_vec::<FlushNoGap>(&mut mp3)
        .map_err(|e| format!("flush encoder gagal: {e:?}"))?;

    // Sisipkan tag LAME/Xing (info VBR + encoder delay) di awal stream supaya
    // player bisa menghitung durasi & seek dengan benar.
    if encoder.is_lame_tag_written() && encoder.lame_tag_size() > 0 {
        let mut tag: Vec<u8> = Vec::new();
        tag.reserve(encoder.lame_tag_size());
        if let Some(written) = encoder.lame_tag_encode_to_vec(&mut tag) {
            let split = encoder.id3v2_tag_size().min(mp3.len());
            let mut merged = Vec::with_capacity(mp3.len() + written.get());
            merged.extend_from_slice(&mp3[..split]);
            merged.extend_from_slice(&tag[..written.get()]);
            merged.extend_from_slice(&mp3[split..]);
            mp3 = merged;
        }
    }

    std::fs::write(output, &mp3).map_err(|e| format!("tulis {} gagal: {e}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_maps_ui_values() {
        assert_eq!(parse_mode("mp3_same"), Some(Mp3Mode::SameBitrate));
        assert_eq!(parse_mode("MP3"), Some(Mp3Mode::SameBitrate));
        assert_eq!(parse_mode(" mp3_vbr0 "), Some(Mp3Mode::Vbr0));
        assert_eq!(parse_mode("V0"), Some(Mp3Mode::Vbr0));
        // "original"/lain-lain = tanpa konversi (FLAC/AAC disimpan apa adanya).
        assert_eq!(parse_mode("original"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn snap_kbps_follows_source_and_caps_at_320() {
        assert_eq!(snap_kbps(Some(96)), 96);
        assert_eq!(snap_kbps(Some(320)), 320);
        // FLAC 16/44.1 (~1411 kbps) → batas tertinggi MP3.
        assert_eq!(snap_kbps(Some(1411)), 320);
        // Bitrate di antara dua nilai valid dibulatkan ke yang terdekat.
        assert_eq!(snap_kbps(Some(250)), 256);
        // Tanpa info bitrate → 320 kbps.
        assert_eq!(snap_kbps(None), 320);
    }

    #[test]
    fn lame_out_rate_handles_hires_rates() {
        assert_eq!(lame_out_rate(44_100).map(NonZeroU32::get), Some(44_100));
        assert_eq!(lame_out_rate(48_000).map(NonZeroU32::get), Some(48_000));
        assert_eq!(lame_out_rate(22_050).map(NonZeroU32::get), Some(22_050));
        // Hi-Res: diturunkan ke rate "keluarga" terdekat yang didukung LAME.
        assert_eq!(lame_out_rate(96_000).map(NonZeroU32::get), Some(48_000));
        assert_eq!(lame_out_rate(88_200).map(NonZeroU32::get), Some(44_100));
        assert_eq!(lame_out_rate(192_000).map(NonZeroU32::get), Some(48_000));
    }

    /// WAV PCM 16-bit stereo berisi sine 440 Hz — dipakai sebagai sumber uji.
    fn write_sine_wav(path: &Path, rate: u32, seconds: f32) {
        let frames = (rate as f32 * seconds) as u32;
        let mut pcm: Vec<u8> = Vec::with_capacity(frames as usize * 4);
        for i in 0..frames {
            let t = i as f32 / rate as f32;
            let value = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            let sample = (value * i16::MAX as f32) as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let data_len = pcm.len() as u32;
        let mut wav: Vec<u8> = Vec::with_capacity(pcm.len() + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // panjang chunk fmt
        wav.extend_from_slice(&1u16.to_le_bytes()); // format PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // 2 channel
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
        wav.extend_from_slice(&4u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bit per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&pcm);
        std::fs::write(path, wav).expect("tulis wav");
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tidal-donlot-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("buat folder temp");
        dir
    }

    #[test]
    fn converts_audio_to_mp3_cbr_and_vbr0() {
        let dir = temp_dir("mp3");
        let wav = dir.join("sine.wav");
        write_sine_wav(&wav, 44_100, 0.5);

        let kbps = probe_bitrate_kbps(&wav);
        assert!(kbps.unwrap_or(0) > 300, "bitrate wav tidak terbaca: {kbps:?}");

        for (name, mode) in [("cbr.mp3", Mp3Mode::SameBitrate), ("vbr0.mp3", Mp3Mode::Vbr0)] {
            let out = dir.join(name);
            convert_to_mp3(&wav, &out, mode, kbps).expect("konversi ke MP3 harus berhasil");
            let bytes = std::fs::read(&out).expect("baca hasil konversi");
            assert!(bytes.len() > 2_000, "{name} terlalu kecil: {} byte", bytes.len());
            // Frame MPEG audio selalu diawali sync word 11 bit (0xFFE…).
            assert_eq!(bytes[0], 0xFF, "{name} bukan stream MP3");
            assert_eq!(bytes[1] & 0xE0, 0xE0, "{name} bukan stream MP3");
            let tagged = Probe::open(&out)
                .expect("buka mp3")
                .guess_file_type()
                .expect("deteksi mp3")
                .read()
                .expect("baca mp3");
            let reported = tagged.properties().audio_bitrate().unwrap_or(0);
            assert!(reported > 0, "{name} tidak terbaca sebagai MP3 oleh lofty");
            match mode {
                // "bitrate sama": WAV 16/44.1 (~1411 kbps) → MP3 320 kbps.
                // lofty menghitung rata-rata (termasuk frame Xing + padding),
                // jadi nilainya bisa sedikit di atas 320.
                Mp3Mode::SameBitrate => assert!(
                    (300..=360).contains(&reported),
                    "{name}: bitrate CBR harus ~320 kbps, dapat {reported}"
                ),
                // VBR: bitrate rata-rata tergantung materi, asal masuk rentang MP3.
                Mp3Mode::Vbr0 => assert!(
                    (32..=320).contains(&reported),
                    "{name}: bitrate VBR di luar rentang MP3: {reported}"
                ),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
