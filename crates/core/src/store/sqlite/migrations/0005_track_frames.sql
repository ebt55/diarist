-- 0005: the sample frames of each channel of a track before the shorter one
-- was padded (plan 16, section 12.5 J), as a JSON list in channel order
-- ([mic, loopback] for the stereo playback FLAC); NULL when unknown, as on
-- every track registered before this. The stereo FLAC pads the shorter
-- channel with silence; running the post-pass again from it cuts each
-- channel back to its own length, so the last word's end clamp is the
-- original one.
ALTER TABLE audio_tracks ADD COLUMN channel_frames TEXT;
