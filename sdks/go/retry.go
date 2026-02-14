package valka

import (
	"math"
	"math/rand"
	"time"
)

// RetryPolicy provides exponential backoff with jitter for reconnection.
type RetryPolicy struct {
	InitialDelay time.Duration
	MaxDelay     time.Duration
	Multiplier   float64
	attempt      int
}

// DefaultRetryPolicy returns a retry policy with sensible defaults.
func DefaultRetryPolicy() *RetryPolicy {
	return &RetryPolicy{
		InitialDelay: 100 * time.Millisecond,
		MaxDelay:     30 * time.Second,
		Multiplier:   2.0,
	}
}

// NextDelay calculates the next backoff delay and increments the attempt counter.
func (r *RetryPolicy) NextDelay() time.Duration {
	delay := float64(r.InitialDelay) * math.Pow(r.Multiplier, float64(r.attempt))
	capped := math.Min(delay, float64(r.MaxDelay))
	jitter := capped * 0.1 * rand.Float64()
	r.attempt++
	return time.Duration(capped + jitter)
}

// Reset resets the attempt counter (call on successful connection).
func (r *RetryPolicy) Reset() {
	r.attempt = 0
}
