<script lang="ts">
  import { formatClock, msToNextSecond } from "../../shared/clock";

  let now = $state(new Date());

  // Each tick reads the time rather than adding a second, so a timer the
  // system delays catches up instead of drifting.
  $effect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const tick = (): void => {
      now = new Date();
      timer = setTimeout(tick, msToNextSecond(Date.now()));
    };
    timer = setTimeout(tick, msToNextSecond(Date.now()));
    return () => clearTimeout(timer);
  });
</script>

<time class="station-clock" datetime={now.toISOString()}>
  {formatClock(now)}
</time>
