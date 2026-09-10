"""Telemetry field contract; unavailable measurements remain null."""
REQUIRED_FIELDS = (
    "timestamp", "run_id", "architecture", "seed", "training_step", "episodes",
    "reward_mean", "reward_std", "task_success", "llm_call_rate",
    "required_llm_call_rate", "missed_llm_rate", "false_llm_call_rate",
    "action_distribution", "state_retention_score", "habituation_score",
    "novelty_response_score", "OOD_score", "steps_per_second", "episodes_per_second",
)
