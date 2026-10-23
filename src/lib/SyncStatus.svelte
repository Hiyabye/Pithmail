<script lang="ts">
  import type { SyncStatus } from './api';
  let {
    status,
    busy,
    onstart,
    onpause,
  }: {
    status: SyncStatus;
    busy: boolean;
    onstart: () => void;
    onpause: () => void;
  } = $props();
</script>

<section class="sync-panel" aria-label="Gmail download status">
  <div class="sync-heading">
    <h3>Mailbox download</h3>
    <span role="status"
      >{status.paused
        ? 'Paused'
        : status.complete
          ? 'Up to date'
          : status.phase.replaceAll('_', ' ')}</span
    >
  </div>
  <p class="muted sync-counts">
    {status.downloaded.toLocaleString()} stored · {status.discovered.toLocaleString()}
    discovered<br />
    {status.pendingBodies.toLocaleString()} pending bodies · {status.pendingAttachments.toLocaleString()}
    pending attachments
  </p>
  {#if status.error}<p class="sync-error" role="alert">{status.error}</p>{/if}
  {#if status.nextRetryAt}
    <p class="muted">
      Next attempt no earlier than {new Date(
        status.nextRetryAt * 1000,
      ).toLocaleString()}.
    </p>
  {/if}
  <div class="button-row">
    {#if !status.paused && !status.complete && !status.error}
      <button disabled={busy} onclick={onpause}>Pause download</button>
    {:else}
      <button disabled={busy} onclick={onstart}
        >{status.error
          ? 'Retry synchronization'
          : status.complete
            ? 'Sync now'
            : 'Resume download'}</button
      >
    {/if}
  </div>
  <p class="muted">
    Bodies appear as they arrive. Full completion includes every required
    attachment. Counts may change as Gmail changes; discovered messages are not
    a fixed total.
  </p>
</section>
