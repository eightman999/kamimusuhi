"""Numeric-only Edge interface; names are metadata, never model inputs."""
SENSOR_NAMES = (
    "motion", "motion_delta", "sound_energy", "light_level", "proximity",
    "touch", "temperature_delta", "user_presence", "speech_activity",
    "repetition", "novelty", "time_since_event", "sensor_confidence",
    "resource_level", "task_pressure", "external_change",
)
ACTION_NAMES = ("IGNORE", "WAIT", "ORIENT", "OBSERVE", "RECALL", "INVOKE_LANGUAGE")
IGNORE, WAIT, ORIENT, OBSERVE, RECALL, INVOKE_LANGUAGE = range(6)
SCENARIO_NAMES = (
    "idle_noise", "repeated_stimulus", "sudden_novelty", "delayed_cue",
    "conflicting_goal", "language_required", "persistent_anomaly", "false_language_trigger",
)
