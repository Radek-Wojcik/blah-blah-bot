import numpy as np

from bbb_tts import audio


def test_trim_removes_silence_but_keeps_padding():
    rate = 16000
    wave = np.zeros(rate * 3, np.float32)
    wave[rate : 2 * rate] = 0.3 * np.sin(np.linspace(0, 2000, rate)).astype(np.float32)
    out = audio.trim_silence(wave, rate, pad_s=0.04)
    assert rate <= len(out) <= rate + int(0.1 * rate)


def test_trim_all_silent_is_unchanged():
    wave = np.zeros(1000, np.float32)
    assert len(audio.trim_silence(wave, 16000)) == 1000


def test_resample_length():
    wave = np.random.default_rng(0).standard_normal(24000).astype(np.float32)
    assert len(audio.resample(wave, 24000, 16000)) == 16000


def test_match_level_never_clips():
    wave = np.array([0.0, 1.0, -1.0, 0.0], np.float32)
    assert np.abs(audio.match_level(wave)).max() <= 0.97


def test_wav_roundtrip_is_16bit_mono(tmp_path):
    import soundfile as sf

    path = tmp_path / "a.wav"
    audio.write_wav(path, np.full(160, 0.1, np.float32))
    info = sf.info(str(path))
    assert (info.samplerate, info.channels, info.subtype) == (16000, 1, "PCM_16")
