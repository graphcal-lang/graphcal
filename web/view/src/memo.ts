/**
 * `load` run once and its promise shared, until it rejects: the next call
 * then runs it again, so a failure (a dropped connection, say) is retried
 * instead of remembered.
 */
export function sharedUntilRejected<T>(load: () => Promise<T>): () => Promise<T> {
  let shared: Promise<T> | null = null;
  return () => {
    if (shared === null) {
      const attempt = load();
      shared = attempt;
      attempt.catch(() => {
        if (shared === attempt) shared = null;
      });
    }
    return shared;
  };
}
