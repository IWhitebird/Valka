export interface RetryPolicyOptions {
  initialDelayMs?: number;
  maxDelayMs?: number;
  multiplier?: number;
}

export class RetryPolicy {
  private initialDelayMs: number;
  private maxDelayMs: number;
  private multiplier: number;
  private attempt = 0;

  constructor(options: RetryPolicyOptions = {}) {
    this.initialDelayMs = options.initialDelayMs ?? 100;
    this.maxDelayMs = options.maxDelayMs ?? 30_000;
    this.multiplier = options.multiplier ?? 2.0;
  }

  nextDelay(): number {
    const delayMs = this.initialDelayMs * Math.pow(this.multiplier, this.attempt);
    const capped = Math.min(delayMs, this.maxDelayMs);
    const jitter = capped * 0.1 * Math.random();
    this.attempt++;
    return Math.floor(capped + jitter);
  }

  reset(): void {
    this.attempt = 0;
  }
}
