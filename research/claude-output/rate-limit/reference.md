# `rate_limit_event`

Emitted once, immediately after `system` init.

## Key fields

| Field                            | Type   | Description                                      |
| -------------------------------- | ------ | ------------------------------------------------ |
| `rate_limit_info.status`         | string | `"allowed"` or `"rate_limited"`                  |
| `rate_limit_info.resetsAt`       | number | Unix timestamp when the rate limit window resets |
| `rate_limit_info.rateLimitType`  | string | e.g. `"five_hour"`                               |
| `rate_limit_info.overageStatus`  | string | `"rejected"`, `"allowed"`                        |
| `rate_limit_info.isUsingOverage` | bool   | Whether overage billing is active                |

## Notes

- This event is informational.
