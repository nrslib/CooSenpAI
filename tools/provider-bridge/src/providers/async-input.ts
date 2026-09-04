export class AsyncInput<T> implements AsyncIterable<T> {
  private readonly values: Array<PendingValue<T>> = [];
  private readonly waiters: Array<(value?: PendingValue<T>) => void> = [];
  private readonly inFlight: Array<PendingValue<T>> = [];
  private closed = false;

  push(value: T): Promise<boolean> {
    if (this.closed) return Promise.resolve(false);
    return new Promise<boolean>((resolve) => {
      const pending = { value, acknowledge: once(resolve) };
      const waiter = this.waiters.shift();
      if (waiter === undefined) this.values.push(pending);
      else waiter(pending);
    });
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.inFlight.splice(0)) pending.acknowledge(false);
    for (const pending of this.values.splice(0)) pending.acknowledge(false);
    for (const waiter of this.waiters.splice(0)) waiter();
  }

  acknowledge(predicate: (value: T) => boolean): boolean {
    const index = this.inFlight.findIndex((pending) => predicate(pending.value));
    if (index < 0) return false;
    const [pending] = this.inFlight.splice(index, 1);
    pending?.acknowledge(true);
    return true;
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return {
      next: () => {
        const pending = this.values.shift();
        if (pending !== undefined) return Promise.resolve(this.consume(pending));
        if (this.closed) return Promise.resolve({ done: true, value: undefined });
        return new Promise<IteratorResult<T>>((resolve) => {
          this.waiters.push((value) => resolve(value === undefined
            ? { done: true, value: undefined }
            : this.consume(value)));
        });
      },
      return: () => {
        this.close();
        return Promise.resolve({ done: true, value: undefined });
      },
    };
  }

  private consume(pending: PendingValue<T>): IteratorResult<T> {
    this.inFlight.push(pending);
    return { done: false, value: pending.value };
  }
}

interface PendingValue<T> {
  readonly value: T;
  readonly acknowledge: (accepted: boolean) => void;
}

function once(callback: (accepted: boolean) => void): (accepted: boolean) => void {
  let called = false;
  return (accepted) => {
    if (called) return;
    called = true;
    callback(accepted);
  };
}
