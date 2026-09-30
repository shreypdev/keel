/**
 * The call id that follows `current` (SPEC 1.3): call ids increase, wrap
 * around at 2^32, never take the reserved value 0, and never collide with a
 * call that is still in flight.
 */
export function nextCallId(current: number, inFlight: (id: number) => boolean): number {
  let id = current;
  do {
    id = (id + 1) >>> 0;
    if (id === 0) id = 1;
  } while (inFlight(id));
  return id;
}
