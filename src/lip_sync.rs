use gdnative::{
    api::{AudioEffect, AudioServer},
    prelude::*,
};

use crate::common::LipSyncInfo;
use crate::lip_sync_job::*;
use crate::profile::*;

#[derive(NativeClass)]
#[inherit(Reference)]
#[user_data(user_data::RwLockData<LipSync>)]
#[register_with(Self::register_lip_sync)]
pub struct LipSync {
    // Godot-specific stuff
    effect: Option<Ref<AudioEffect, Shared>>,

    // Unity stuff
    pub profile: Profile,
    pub output_sound_gain: f64,

    index: i64,

    raw_input_data: Vec<f64>,
    input_data: Vec<f64>,
    mfcc: Vec<f64>,
    mfcc_for_other: Vec<f64>,
    phonemes: Vec<f64>,
    job_result: Vec<LipSyncJobResult>,
    requested_calibration_vowels: Vec<i64>,

    result: LipSyncInfo,
}

#[methods]
impl LipSync {
    fn new(_owner: &Reference) -> Self {
        LipSync {
            effect: None,

            profile: Profile::new(),
            output_sound_gain: 1.0,

            index: 0,

            raw_input_data: vec![],
            input_data: vec![],
            mfcc: vec![],
            mfcc_for_other: vec![],
            phonemes: vec![],
            job_result: vec![],
            requested_calibration_vowels: vec![],

            result: LipSyncInfo::default(),
        }
    }

    fn register_lip_sync(builder: &ClassBuilder<Self>) {
        builder.add_signal(Signal {
            name: "lip_sync_updated",
            args: &[SignalArgument {
                name: "result",
                default: Variant::from_dictionary(&Dictionary::default()),
                export_info: ExportInfo::new(VariantType::Dictionary),
                usage: PropertyUsage::DEFAULT,
            }],
        })
    }

    // Maps to Awake() in the Unity impl
    #[export]
    fn _init(&mut self, _owner: &Reference) {
        self.update_audio_source();
    }

    // Maps to Update() in the Unity impl
    #[export]
    fn _process(&mut self, owner: &Reference) {
        //

        self.update_result();
        self.invoke_callback(owner);
        self.update_calibration();
        self.update_phonemes();
        self.schedule_job();

        self.update_buffers();
        self.update_audio_source();
    }

    fn awake() {
        unimplemented!("Unity-specific")
    }

    fn on_enable() {
        unimplemented!("Unity-specific")
    }

    fn on_disable() {
        unimplemented!("Unity-specific")
    }

    fn allocate_buffers(&mut self) {
        self.raw_input_data = vec![];
        self.input_data = vec![];
        self.mfcc = vec![];
        self.mfcc_for_other = vec![];
        self.job_result = vec![];
        self.phonemes = vec![];
    }

    fn dispose_buffers(&mut self) {
        self.raw_input_data.clear();
        self.input_data.clear();
        self.mfcc.clear();
        self.mfcc_for_other.clear();
        self.job_result.clear();
        self.phonemes.clear();
    }

    fn update_buffers(&mut self) {
        if self.input_sample_count() != self.raw_input_data.len() as i64
            || self.profile.mfccs.len() * 12 != self.phonemes.len()
        {
            self.dispose_buffers();
            self.allocate_buffers();
        }
    }

    fn update_result(&mut self) {
        // wait for thread to complete
        // TODO stub

        self.mfcc_for_other.copy_from_slice(&self.mfcc);

        // TODO hopefully they're not just using lists as their main data structure
        let index = self.job_result[0].index;
        let phoneme = self.profile.get_phoneme(index as usize);
        let distance = self.job_result[0].distance;
        let mut vol = self.job_result[0].volume.log10();
        let min_vol = self.profile.min_volume;
        let max_vol = self.profile.max_volume.max(min_vol + 1e-4_f64);
        vol = (vol - min_vol) / (max_vol - min_vol);
        vol = f64::clamp(vol, 0.0, 1.0);

        self.result = LipSyncInfo::new(index, phoneme, vol, self.job_result[0].volume, distance);
    }

    fn invoke_callback(&mut self, owner: &Reference) {
        owner.emit_signal(
            "lip_sync_updated",
            &[Variant::from_dictionary(&self.result())],
        );
    }

    fn update_phonemes(&mut self) {
        let mut index: usize = 0;
        for data in self.profile.mfccs.iter() {
            for value in data.mfcc_native_array.iter() {
                if index >= self.phonemes.len() {
                    break;
                }
                index += 1;
                self.phonemes[index] = *value;
            }
        }
    }

    fn schedule_job(&mut self) {
        // TODO incomplete, this is the hard part
        let mut index: i64 = 0;

        self.input_data.clone_from(&self.raw_input_data);
        index = self.index;

        // TODO cloning for now, we might actually need a reference
        let job = LipSyncJob {
            input: self.input_data.clone(),
            start_index: index,
            output_sample_rate: AudioServer::godot_singleton().get_mix_rate() as i64,
            target_sample_rate: self.profile.target_sample_rate,
            volume_thresh: (10.0 as f64).powf(self.profile.min_volume),
            mel_filter_bank_channels: self.profile.mel_filter_bank_channels,
            mfcc: self.mfcc.clone(),
            phonemes: self.phonemes.clone(),
            result: self.job_result.clone(),
        };

        // TODO run on thread
    }

    #[export]
    pub fn request_calibration(&mut self, _owner: &Reference, index: i64) {
        if index < 0 {
            return;
        }
        self.requested_calibration_vowels.push(index);
    }

    fn update_calibration(&mut self) {
        for index in self.requested_calibration_vowels.iter() {
            // We can assume index is greater than 0 because we check
            // for this in request_calibration
            self.profile
                .update_mfcc(*index as usize, self.mfcc.clone(), true);
        }

        self.requested_calibration_vowels.clear();
    }

    fn update_audio_source(&mut self) {
        let audio_server = AudioServer::godot_singleton();
        let record_effect_index = audio_server.get_bus_index("Record");
        self.effect = audio_server.get_bus_effect(record_effect_index, 0);
    }

    // TODO connect to some audio thing
    // https://github.com/godot-rust/godot-rust/blob/0.9.3/examples/signals/src/lib.rs#L73
    fn on_data_received(&mut self, _owner: &Reference, input: &mut TypedArray<f32>, channels: i64) {
        if self.raw_input_data.len() == 0 {
            return;
        }

        let n = self.raw_input_data.len() as i64;
        self.index = self.index % n;
        let mut i = 0;
        while i < input.len() {
            self.index = (self.index + 1) % n;
            self.raw_input_data[self.index as usize] = input.get(i as i32).into();

            i += channels as i32;
        }

        if (self.output_sound_gain - 1.0).abs() > f64::EPSILON {
            let n = input.len() as i32;
            for i in 0..n {
                input.set(i, input.get(i) * self.output_sound_gain as f32);
            }
        }
    }

    fn on_audio_filter_read() {
        // TODO this might not be true
        unimplemented!("Unity-specific")
    }

    // Changed from property in the Unity impl to function
    // TODO might need to convert to Godot Array
    pub fn mfcc(&self) -> &Vec<f64> {
        &self.mfcc_for_other
    }

    // Changed from property in the Unity impl to function
    pub fn result(&self) -> Dictionary {
        let dict = Dictionary::new();
        dict.insert("index", self.result.index);
        dict.insert("phoneme", self.result.phoneme.clone());
        dict.insert("volume", self.result.volume);
        dict.insert("raw_volume", self.result.raw_volume);
        dict.insert("distance", self.result.distance);

        dict.into_shared()
    }

    // Changed from property in the Unity impl to function
    fn input_sample_count(&self) -> i64 {
        let r =
            AudioServer::godot_singleton().get_mix_rate() / self.profile.target_sample_rate as f64;
        (self.profile.sample_count as f64 * r).ceil() as i64
    }
}