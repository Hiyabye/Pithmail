<script lang="ts">
  import type { MessageDetail } from './api';

  let {
    detail,
    thread,
    loading,
    hasMore,
    loadingMore,
    onmore,
    onselect,
    onclose,
  }: {
    detail: MessageDetail | null;
    thread: MessageDetail[];
    loading: boolean;
    hasMore: boolean;
    loadingMore: boolean;
    onmore: () => void;
    onselect: (id: string) => void;
    onclose: () => void;
  } = $props();
</script>

<section class="reader" aria-label="Message reader" aria-busy={loading}>
  {#if loading && !detail}
    <p class="empty-state" role="status">Opening local message…</p>
  {:else if detail}
    <div class="reader-toolbar">
      <span>Original message · plain text</span><button
        class="text-button"
        onclick={onclose}>Close message</button
      >
    </div>
    <article>
      <header class="reader-header">
        <h2>{detail.message.subject}</h2>
        <p class="reader-sender">{detail.message.sender}</p>
        <p class="muted">
          <time
            datetime={new Date(detail.message.receivedAt * 1000).toISOString()}
            >{new Date(detail.message.receivedAt * 1000).toLocaleString()}</time
          >
        </p>
        <p class="message-labels">{detail.message.labels.join(' · ')}</p>
      </header>
      <div class="mail-body" dir="auto">
        {detail.body ||
          '(No readable text is available yet. If synchronization is in progress, this body may still be downloading.)'}
      </div>
      {#if detail.attachments.length > 0}
        <section class="attachments" aria-label="Attachments">
          <h3>Attachments</h3>
          <p class="muted">
            Attachment metadata is available before downloads finish. Check sync
            settings for pending downloads. Opening and exporting attachments is
            not available in this milestone.
          </p>
          <ul>
            {#each detail.attachments as attachment, index (`${attachment.name}-${index}`)}
              <li>
                <span>{attachment.name}</span><span class="muted"
                  >{attachment.size.toLocaleString()} bytes</span
                >
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    </article>
    {#if thread.length > 1 || hasMore}
      <nav class="thread-nav" aria-label="Messages in this thread">
        <h3>This conversation <span class="muted">({thread.length})</span></h3>
        {#each thread as item (item.message.id)}
          <button
            class="thread-item"
            aria-current={item.message.id === detail.message.id
              ? 'true'
              : undefined}
            onclick={() => onselect(item.message.id)}
          >
            <span>{item.message.sender}</span><span class="muted"
              >{item.message.subject}</span
            >
          </button>
        {/each}
        {#if hasMore}<button disabled={loadingMore} onclick={onmore}
            >{loadingMore
              ? 'Loading conversation…'
              : 'Load more conversation messages'}</button
          >{/if}
      </nav>
    {/if}
  {:else}
    <div class="reader-placeholder">
      <h2>A little less inbox.</h2>
      <p>
        Select a message to read it here.<br />Your mail stays on this device.
      </p>
    </div>
  {/if}
</section>
