//! Native decoding and output. HTTP and codecs never run on the device callback.
use crate::{
    audio_settings::AudioSettings,
    playback::{AudioSource, PlaybackEvent, PlayerEvent},
};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CodecRegistry, DecoderOptions},
    formats::{FormatOptions, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
    units::Time,
};

pub(crate) enum Command {
    Load(AudioSource),
    Pause(bool),
    Seek(f64),
    Volume(f64),
    Configure(AudioSettings),
}
struct Shared {
    pcm: Mutex<VecDeque<[f32; 2]>>,
    paused: AtomicBool,
    volume: AtomicU64,
    consumed: AtomicU64,
    eof: AtomicBool,
    output_failed: AtomicBool,
    dsp: Mutex<(bool, [f64; 5])>,
}
pub(crate) struct NativeSession {
    commands: Sender<Command>,
    worker: Option<thread::JoinHandle<()>>,
    pub generation: u64,
    pub started: Instant,
    pub loaded: Arc<AtomicBool>,
    pub closing: Arc<AtomicBool>,
}
impl NativeSession {
    pub fn start(events: Sender<PlaybackEvent>, generation: u64) -> Result<Self> {
        let (commands, requests) = mpsc::channel();
        let (initialized, ready) = mpsc::sync_channel(1);
        let loaded = Arc::new(AtomicBool::new(false));
        let closing = Arc::new(AtomicBool::new(false));
        let worker_loaded = loaded.clone();
        let worker_closing = closing.clone();
        let worker = thread::spawn(move || {
            let shared = Arc::new(Shared {
                pcm: Mutex::new(VecDeque::with_capacity(192_000)),
                paused: AtomicBool::new(true),
                volume: AtomicU64::new(0.7f64.to_bits()),
                consumed: AtomicU64::new(0),
                eof: AtomicBool::new(false),
                output_failed: AtomicBool::new(false),
                dsp: Mutex::new((false, [0.; 5])),
            });
            let emit = |event| {
                let _ = events.send(PlaybackEvent { generation, event });
            };
            emit(PlayerEvent::Devices(devices()));
            let mut output = match open_output("auto", shared.clone()) {
                Ok(value) => {
                    let _ = initialized.send(Ok(()));
                    value
                }
                Err(error) => {
                    let _ = initialized.send(Err(format!("{error:#}")));
                    return;
                }
            };
            let mut source: Option<AudioSource> = None;
            let mut cancellation = Arc::new(AtomicBool::new(false));
            let (decoded, results) = mpsc::channel::<(u64, Result<f64>)>();
            let mut epoch = 0;
            let mut base = 0.;
            let mut duration = 0.;
            let mut announced = false;
            let mut ended = false;
            let mut buffering = false;
            let mut selected = String::from("auto");
            let mut tick = Instant::now();
            let restart = |source: AudioSource,
                           seconds: f64,
                           rate: u32,
                           epoch: u64,
                           cancellation: Arc<AtomicBool>| {
                let shared = shared.clone();
                let decoded = decoded.clone();
                thread::spawn(move || {
                    let result =
                        decode(source, seconds, rate, &shared, &cancellation, |duration| {
                            let _ = decoded.send((epoch, Ok(duration)));
                        });
                    if !cancellation.load(Ordering::Acquire) {
                        let _ = decoded.send((epoch, result));
                    }
                });
            };
            while !worker_closing.load(Ordering::Acquire) {
                let wait = if source.is_none() || shared.paused.load(Ordering::Acquire) && announced
                {
                    Duration::from_millis(200)
                } else {
                    Duration::from_millis(20)
                };
                let command = requests.recv_timeout(wait);
                let mut reload = false;
                match command {
                    Ok(Command::Load(value)) => {
                        source = Some(value);
                        duration = 0.;
                        base = 0.;
                        reload = true;
                    }
                    Ok(Command::Seek(seconds)) => {
                        if seconds.is_finite() {
                            if duration > 0. && seconds >= duration {
                                cancellation.store(true, Ordering::Release);
                                shared.pcm.lock().unwrap().clear();
                                source = None;
                                emit(PlayerEvent::Position(duration));
                                emit(PlayerEvent::Ended);
                                continue;
                            }
                            base = seconds.max(0.);
                            reload = true;
                        }
                    }
                    Ok(Command::Pause(value)) => {
                        shared.paused.store(value, Ordering::Release);
                        emit(PlayerEvent::Paused(value));
                    }
                    Ok(Command::Volume(value)) => {
                        shared
                            .volume
                            .store((value.clamp(0., 100.) / 100.).to_bits(), Ordering::Release);
                    }
                    Ok(Command::Configure(settings)) => {
                        *shared.dsp.lock().unwrap() = (settings.normalize, settings.equalizer);
                        if settings.output != selected {
                            let position = base
                                + shared.consumed.load(Ordering::Acquire) as f64 / output.1 as f64;
                            // Release the old stream before opening an exclusive device.
                            let old_rate = output.1;
                            drop(output.0);
                            match open_output(&settings.output, shared.clone()) {
                                Ok(value) => {
                                    output = value;
                                    selected = settings.output;
                                }
                                Err(error) => {
                                    emit(PlayerEvent::SettingsError(format!(
                                        "{error:#}. Using system output."
                                    )));
                                    match open_output("auto", shared.clone()) {
                                        Ok(value) => {
                                            output = value;
                                            selected = "auto".into();
                                        }
                                        Err(error) => {
                                            emit(PlayerEvent::Error(format!("{error:#}")));
                                            break;
                                        }
                                    }
                                }
                            }
                            // Rate changes require resampling again from the current media time.
                            if old_rate != output.1 {
                                base = position;
                                reload = source.is_some();
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if shared.output_failed.swap(false, Ordering::AcqRel) {
                    base += shared.consumed.load(Ordering::Acquire) as f64 / output.1 as f64;
                    drop(output.0);
                    match open_output("auto", shared.clone()) {
                        Ok(value) => {
                            output = value;
                            selected = "auto".into();
                            reload = source.is_some();
                            emit(PlayerEvent::Devices(devices()));
                        }
                        Err(error) => {
                            emit(PlayerEvent::Error(format!(
                                "Audio output disconnected: {error:#}"
                            )));
                            break;
                        }
                    }
                }
                if reload && let Some(source) = source.clone() {
                    cancellation.store(true, Ordering::Release);
                    cancellation = Arc::new(AtomicBool::new(false));
                    epoch += 1;
                    shared.pcm.lock().unwrap().clear();
                    shared.consumed.store(0, Ordering::Release);
                    shared.eof.store(false, Ordering::Release);
                    worker_loaded.store(false, Ordering::Release);
                    announced = false;
                    ended = false;
                    buffering = false;
                    emit(PlayerEvent::Position(base));
                    restart(source, base, output.1, epoch, cancellation.clone());
                }
                while let Ok((id, result)) = results.try_recv() {
                    if id != epoch {
                        continue;
                    }
                    match result {
                        Ok(value) => {
                            if value > 0. {
                                duration = value;
                                emit(PlayerEvent::Duration(duration));
                            }
                        }
                        Err(error) => {
                            emit(PlayerEvent::Error(format!("{error:#}")));
                            source = None;
                        }
                    }
                }
                if source.is_some() {
                    let buffered = shared.pcm.lock().unwrap().len();
                    if !announced
                        && (buffered >= output.1 as usize / 10
                            || shared.eof.load(Ordering::Acquire)
                                && (buffered > 0 || shared.consumed.load(Ordering::Acquire) > 0))
                    {
                        announced = true;
                        worker_loaded.store(true, Ordering::Release);
                        emit(PlayerEvent::Loaded);
                        emit(PlayerEvent::Buffering(false));
                        emit(PlayerEvent::Paused(shared.paused.load(Ordering::Acquire)));
                    }
                    if announced && !ended && shared.eof.load(Ordering::Acquire) && buffered == 0 {
                        ended = true;
                        emit(PlayerEvent::Ended);
                    }
                    let starved = announced
                        && !ended
                        && !shared.paused.load(Ordering::Acquire)
                        && !shared.eof.load(Ordering::Acquire)
                        && buffered == 0;
                    if starved != buffering {
                        buffering = starved;
                        emit(PlayerEvent::Buffering(buffering));
                    }
                    if announced
                        && !ended
                        && !shared.paused.load(Ordering::Acquire)
                        && tick.elapsed() >= Duration::from_millis(100)
                    {
                        tick = Instant::now();
                        emit(PlayerEvent::Position(
                            base + shared.consumed.load(Ordering::Acquire) as f64 / output.1 as f64,
                        ));
                    }
                }
            }
            cancellation.store(true, Ordering::Release);
            worker_closing.store(true, Ordering::Release);
        });
        match ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                commands,
                worker: Some(worker),
                generation,
                started: Instant::now(),
                loaded,
                closing,
            }),
            result => {
                closing.store(true, Ordering::Release);
                bail!(
                    "{}",
                    result
                        .ok()
                        .and_then(Result::err)
                        .unwrap_or_else(|| "Audio device did not respond".into())
                );
            }
        }
    }
    pub fn send(&mut self, command: Command) -> Result<()> {
        self.commands
            .send(command)
            .context("Native audio engine stopped")
    }
    pub fn is_running(&mut self) -> bool {
        !self.closing.load(Ordering::Acquire)
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        let _ = self.commands.send(Command::Pause(true));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn devices() -> Vec<(String, String)> {
    let mut devices = vec![("auto".into(), "System default".into())];
    if let Ok(outputs) = cpal::default_host().output_devices() {
        devices.extend(
            outputs
                .filter_map(|device| device.name().ok())
                .map(|name| (format!("native:{name}"), name)),
        );
    }
    devices
}
fn open_output(selected: &str, shared: Arc<Shared>) -> Result<(cpal::Stream, u32)> {
    let host = cpal::default_host();
    let device = if selected == "auto" {
        host.default_output_device()
    } else {
        host.output_devices()?
            .find(|device| device.name().ok().as_deref() == selected.strip_prefix("native:"))
    }
    .context("Selected audio output is unavailable")?;
    let config = device.default_output_config()?;
    let rate = config.sample_rate().0;
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => output_stream::<f32>(&device, &config.into(), shared),
        cpal::SampleFormat::F64 => output_stream::<f64>(&device, &config.into(), shared),
        cpal::SampleFormat::I8 => output_stream::<i8>(&device, &config.into(), shared),
        cpal::SampleFormat::I16 => output_stream::<i16>(&device, &config.into(), shared),
        cpal::SampleFormat::I24 => output_stream::<cpal::I24>(&device, &config.into(), shared),
        cpal::SampleFormat::I32 => output_stream::<i32>(&device, &config.into(), shared),
        cpal::SampleFormat::I64 => output_stream::<i64>(&device, &config.into(), shared),
        cpal::SampleFormat::U8 => output_stream::<u8>(&device, &config.into(), shared),
        cpal::SampleFormat::U16 => output_stream::<u16>(&device, &config.into(), shared),
        cpal::SampleFormat::U32 => output_stream::<u32>(&device, &config.into(), shared),
        cpal::SampleFormat::U64 => output_stream::<u64>(&device, &config.into(), shared),
        _ => bail!("Audio output uses an unsupported sample format"),
    }?;
    stream.play()?;
    Ok((stream, rate))
}
fn output_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Result<cpal::Stream> {
    let channels = config.channels as usize;
    let mut dsp = Dsp::new(config.sample_rate.0);
    let failed = shared.clone();
    let mut volume = 0f32;
    Ok(device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            // Never wait on decoding/networking or allocate on the audio thread.
            output.fill(T::from_sample(0.));
            if shared.paused.load(Ordering::Acquire) {
                return;
            }
            let Ok(mut pcm) = shared.pcm.try_lock() else {
                return;
            };
            if let Ok(settings) = shared.dsp.try_lock() {
                dsp.configure(settings.0, settings.1);
            }
            let target = f64::from_bits(shared.volume.load(Ordering::Acquire)) as f32;
            let mut consumed = 0;
            for frame in output.chunks_mut(channels) {
                let Some(stereo) = pcm.pop_front() else {
                    break;
                };
                volume += (target - volume) * 0.002;
                let stereo = dsp.process(stereo);
                if channels == 1 {
                    frame[0] = T::from_sample((stereo[0] + stereo[1]) * 0.5 * volume);
                } else {
                    frame[0] = T::from_sample(stereo[0] * volume);
                    frame[1] = T::from_sample(stereo[1] * volume);
                }
                consumed += 1;
            }
            shared.consumed.fetch_add(consumed, Ordering::Release);
        },
        move |_| {
            failed.output_failed.store(true, Ordering::Release);
        },
        None,
    )?)
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    state: [[f64; 2]; 2],
}
impl Biquad {
    fn peaking(rate: f64, frequency: f64, gain: f64) -> Self {
        let amplitude = 10f64.powf(gain / 40.);
        let omega = 2. * std::f64::consts::PI * frequency.min(rate * 0.45) / rate;
        let alpha = omega.sin() / 2.;
        let cosine = omega.cos();
        let a0 = 1. + alpha / amplitude;
        Self {
            b: [
                (1. + alpha * amplitude) / a0,
                -2. * cosine / a0,
                (1. - alpha * amplitude) / a0,
            ],
            a: [-2. * cosine / a0, (1. - alpha / amplitude) / a0],
            state: [[0.; 2]; 2],
        }
    }
    fn sample(&mut self, input: f64, channel: usize) -> f64 {
        let output = self.b[0] * input + self.state[channel][0];
        self.state[channel][0] = self.b[1] * input - self.a[0] * output + self.state[channel][1];
        self.state[channel][1] = self.b[2] * input - self.a[1] * output;
        output
    }
}
struct Dsp {
    rate: f64,
    gains: [f64; 5],
    filters: [Biquad; 5],
    normalize: bool,
    energy: f64,
    gain: f64,
    headroom: f64,
}
impl Dsp {
    fn new(rate: u32) -> Self {
        Self {
            rate: rate as f64,
            gains: [f64::NAN; 5],
            filters: [Biquad::default(); 5],
            normalize: false,
            energy: 0.01,
            gain: 1.,
            headroom: 1.,
        }
    }
    fn configure(&mut self, normalize: bool, gains: [f64; 5]) {
        self.normalize = normalize;
        if self.gains != gains {
            self.gains = gains;
            let boost = gains.into_iter().fold(0f64, f64::max);
            self.headroom = if boost > 0. {
                10f64.powf(-(boost + 1.) / 20.)
            } else {
                1.
            };
            for (index, frequency) in [60., 230., 910., 3600., 14000.].into_iter().enumerate() {
                let state = self.filters[index].state;
                self.filters[index] = Biquad::peaking(self.rate, frequency, gains[index]);
                self.filters[index].state = state;
            }
        }
    }
    fn process(&mut self, input: [f32; 2]) -> [f32; 2] {
        let power = (input[0] as f64).powi(2) * 0.5 + (input[1] as f64).powi(2) * 0.5;
        self.energy += (power - self.energy) / self.rate;
        let desired = if self.normalize && self.energy > 0.00001 {
            (0.15 / self.energy.sqrt()).clamp(0.25, 6.)
        } else {
            1.
        };
        self.gain += (desired - self.gain) / (self.rate * 0.5);
        let mut output = [0.; 2];
        for channel in 0..2 {
            let mut sample = input[channel] as f64 * self.headroom;
            for filter in &mut self.filters {
                sample = filter.sample(sample, channel);
            }
            output[channel] = (sample * self.gain).clamp(-1., 1.) as f32;
        }
        output
    }
}

fn decode(
    source: AudioSource,
    seconds: f64,
    output_rate: u32,
    shared: &Shared,
    cancelled: &Arc<AtomicBool>,
    metadata: impl Fn(f64),
) -> Result<f64> {
    let media: Box<dyn MediaSource> = if source.url.starts_with("https://") {
        Box::new(HttpSource::new(&source, cancelled.clone())?)
    } else {
        Box::new(std::fs::File::open(&source.url).context("Could not open audio file")?)
    };
    let stream = MediaSourceStream::new(media, Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = std::path::Path::new(source.url.split('?').next().unwrap_or(""))
        .extension()
        .and_then(|ext| ext.to_str())
    {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?
        .format;
    let mut codecs = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut codecs);
    codecs.register_all::<symphonia_adapter_libopus::OpusDecoder>();
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.sample_rate.is_some()
                && codecs
                    .make(&track.codec_params, &DecoderOptions::default())
                    .is_ok()
        })
        .context("No supported audio track")?;
    let id = track.id;
    let params = track.codec_params.clone();
    let time_base = params.time_base;
    let duration = params
        .n_frames
        .and_then(|frames| time_base.map(|base| base.calc_time(frames)))
        .map(|time| time.seconds as f64 + time.frac)
        .unwrap_or(0.);
    metadata(duration);
    let mut decoder = codecs.make(&params, &DecoderOptions::default())?;
    if seconds > 0. {
        format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(seconds),
                    track_id: Some(id),
                },
            )
            .context("This audio source cannot seek")?;
    }
    let mut resampler: Option<SincFixedIn<f32>> = None;
    let mut input_rate = 0;
    let mut pending = [VecDeque::<f32>::new(), VecDeque::new()];
    let mut skip_delay = 0;
    let mut output_count = 0u64;
    let mut input_count = 0u64;
    let push = |frames: &[[f32; 2]]| -> Result<()> {
        for chunk in frames.chunks(1024) {
            loop {
                if cancelled.load(Ordering::Acquire) {
                    bail!("Audio decoding cancelled");
                }
                let mut pcm = shared.pcm.lock().unwrap();
                if cancelled.load(Ordering::Acquire) {
                    bail!("Audio decoding cancelled");
                }
                if pcm.len() + chunk.len() <= output_rate as usize {
                    pcm.extend(chunk);
                    break;
                }
                drop(pcm);
                thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    };
    loop {
        if cancelled.load(Ordering::Acquire) {
            bail!("Audio decoding cancelled");
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(error))
                if error.kind() == io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(audio) => audio,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        let rate = audio.spec().rate;
        let channels = audio.spec().channels.count();
        if rate != input_rate {
            input_rate = rate;
            if rate != output_rate {
                let value = SincFixedIn::new(
                    output_rate as f64 / rate as f64,
                    1.,
                    SincInterpolationParameters {
                        sinc_len: 256,
                        f_cutoff: 0.95,
                        oversampling_factor: 128,
                        interpolation: SincInterpolationType::Cubic,
                        window: WindowFunction::BlackmanHarris2,
                    },
                    1024,
                    2,
                )?;
                skip_delay = value.output_delay();
                resampler = Some(value);
            } else {
                resampler = None;
            }
        }
        let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, *audio.spec());
        samples.copy_interleaved_ref(audio);
        let packet_time = time_base
            .map(|base| base.calc_time(packet.ts()))
            .map(|time| time.seconds as f64 + time.frac)
            .unwrap_or(seconds);
        let discard = ((seconds - packet_time).max(0.) * rate as f64) as usize;
        let stereo: Vec<[f32; 2]> = samples
            .samples()
            .chunks_exact(channels)
            .skip(discard)
            .map(|frame| {
                if channels == 1 {
                    [frame[0], frame[0]]
                } else if channels == 2 {
                    [frame[0], frame[1]]
                } else {
                    let center = frame.get(2).copied().unwrap_or(0.) * 0.707;
                    [(frame[0] + center) * 0.5, (frame[1] + center) * 0.5]
                }
            })
            .collect();
        input_count += stereo.len() as u64;
        if let Some(resampler) = &mut resampler {
            for frame in stereo {
                pending[0].push_back(frame[0]);
                pending[1].push_back(frame[1]);
            }
            while pending[0].len() >= 1024 {
                let block: Vec<Vec<f32>> = pending
                    .iter_mut()
                    .map(|channel| channel.drain(..1024).collect())
                    .collect();
                let output = resampler.process(&block, None)?;
                let discard = skip_delay.min(output[0].len());
                skip_delay -= discard;
                let frames: Vec<_> = output[0]
                    .iter()
                    .zip(&output[1])
                    .skip(discard)
                    .map(|(&l, &r)| [l, r])
                    .collect();
                output_count += frames.len() as u64;
                push(&frames)?;
            }
        } else {
            output_count += stereo.len() as u64;
            push(&stereo)?;
        }
    }
    if let Some(resampler) = &mut resampler {
        let expected = (input_count as f64 * output_rate as f64 / input_rate as f64).round() as u64;
        let mut block: Vec<Vec<f32>> = pending
            .iter_mut()
            .map(|channel| channel.drain(..).collect())
            .collect();
        for channel in &mut block {
            channel.resize(1024, 0.);
        }
        for _ in 0..3 {
            let output = resampler.process(&block, None)?;
            let discard = skip_delay.min(output[0].len());
            skip_delay -= discard;
            let count =
                (expected.saturating_sub(output_count) as usize).min(output[0].len() - discard);
            let frames: Vec<_> = output[0]
                .iter()
                .zip(&output[1])
                .skip(discard)
                .take(count)
                .map(|(&l, &r)| [l, r])
                .collect();
            output_count += frames.len() as u64;
            push(&frames)?;
            if output_count >= expected {
                break;
            }
            for channel in &mut block {
                channel.fill(0.);
            }
        }
    }
    if output_count == 0 {
        bail!("Audio source contained no playable samples");
    }
    if cancelled.load(Ordering::Acquire) {
        bail!("Audio decoding cancelled");
    }
    shared.eof.store(true, Ordering::Release);
    Ok(if duration > 0. {
        0.
    } else {
        seconds + input_count as f64 / input_rate as f64
    })
}

/// Bounded range cache keeps seeks cheap without retaining complete remote songs.
struct HttpSource {
    client: reqwest::blocking::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    position: u64,
    length: Option<u64>,
    blocks: VecDeque<(u64, Vec<u8>)>,
    fallback: Option<std::fs::File>,
    cancelled: Arc<AtomicBool>,
}
impl HttpSource {
    fn new(source: &AudioSource, cancelled: Arc<AtomicBool>) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        for field in &source.headers {
            if let Some((key, value)) = field.split_once(':') {
                let name = reqwest::header::HeaderName::from_bytes(key.trim().as_bytes())?;
                if name != reqwest::header::RANGE && name != reqwest::header::ACCEPT_ENCODING {
                    headers.insert(name, reqwest::header::HeaderValue::from_str(value.trim())?);
                }
            }
        }
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        );
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(4))
                .user_agent(&source.user_agent)
                .build()?,
            url: source.url.clone(),
            headers,
            position: 0,
            length: None,
            blocks: VecDeque::new(),
            fallback: None,
            cancelled,
        })
    }
    fn fetch(&mut self, start: u64) -> io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Audio request cancelled",
            ));
        }
        let mut response = self
            .client
            .get(&self.url)
            .headers(self.headers.clone())
            .header(
                reqwest::header::RANGE,
                format!("bytes={start}-{}", start + 262143),
            )
            .send()
            .map_err(io::Error::other)?;
        if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            if let Some(length) = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes */"))
                .and_then(|v| v.parse().ok())
            {
                self.length = Some(length);
            }
            return Ok(());
        }
        response.error_for_status_ref().map_err(io::Error::other)?;
        if response.status() == reqwest::StatusCode::OK {
            // Some origins ignore Range. Use a private anonymous file, not unbounded RAM.
            let mut file = tempfile::tempfile()?;
            let count = io::copy(&mut response.take(512 * 1024 * 1024 + 1), &mut file)?;
            if count > 512 * 1024 * 1024 {
                return Err(io::Error::other("Audio file exceeds 512 MiB"));
            }
            self.length = Some(count);
            file.seek(SeekFrom::Start(self.position))?;
            self.fallback = Some(file);
            return Ok(());
        }
        let range = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let (span, total) = range
            .strip_prefix("bytes ")
            .and_then(|v| v.split_once('/'))
            .ok_or_else(|| io::Error::other("Invalid audio range response"))?;
        let first = span
            .split_once('-')
            .and_then(|(first, _)| first.parse::<u64>().ok());
        if first != Some(start) {
            return Err(io::Error::other("Audio server returned the wrong range"));
        }
        self.length = total.parse().ok().or(self.length);
        let mut bytes = Vec::new();
        response.by_ref().take(262145).read_to_end(&mut bytes)?;
        if bytes.len() > 262144 {
            return Err(io::Error::other("Audio range exceeds requested size"));
        }
        if self.blocks.len() >= 32 {
            self.blocks.pop_front();
        }
        self.blocks.push_back((start, bytes));
        Ok(())
    }
}
impl Read for HttpSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.length.is_some_and(|length| self.position >= length) {
            return Ok(0);
        }
        if let Some(file) = &mut self.fallback {
            let count = file.read(output)?;
            self.position += count as u64;
            return Ok(count);
        }
        let start = self.position / 262144 * 262144;
        if !self.blocks.iter().any(|(at, _)| *at == start) {
            self.fetch(start)?;
        }
        if self.fallback.is_some() {
            return self.read(output);
        }
        let Some((_, bytes)) = self.blocks.iter().find(|(at, _)| *at == start) else {
            return Ok(0);
        };
        let offset = (self.position - start) as usize;
        if offset >= bytes.len() {
            return Ok(0);
        }
        let count = output.len().min(bytes.len() - offset);
        output[..count].copy_from_slice(&bytes[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for HttpSource {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        if self.length.is_none() && matches!(seek, SeekFrom::End(_)) {
            self.fetch(0)?;
        }
        let position = match seek {
            SeekFrom::Start(value) => value as i128,
            SeekFrom::Current(value) => self.position as i128 + value as i128,
            SeekFrom::End(value) => {
                self.length
                    .ok_or_else(|| io::Error::other("Audio length unavailable"))?
                    as i128
                    + value as i128
            }
        };
        if position < 0 || position > u64::MAX as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid audio seek",
            ));
        }
        self.position = position as u64;
        if let Some(file) = &mut self.fallback {
            file.seek(SeekFrom::Start(self.position))?;
        }
        Ok(self.position)
    }
}
impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.length
    }
}
