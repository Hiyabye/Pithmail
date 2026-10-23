<script lang="ts">
  import type { MessageSummary } from './api';

  let {
    messages,
    selectedId,
    onselect,
    empty = 'No messages to show.',
  }: {
    messages: MessageSummary[];
    selectedId: string | null;
    onselect: (id: string) => void;
    empty?: string;
  } = $props();

  const date = (timestamp: number) =>
    new Date(timestamp * 1000).toLocaleDateString(undefined, {
      month: 'short',
      day: 'numeric',
    });
</script>

{#if messages.length === 0}
  <p class="empty-state">{empty}</p>
{:else}
  <ul class="message-list" aria-label="Messages">
    {#each messages as message (message.id)}
      <li>
        <button
          class="message-item"
          class:selected={message.id === selectedId}
          aria-current={message.id === selectedId ? 'true' : undefined}
          onclick={() => onselect(message.id)}
        >
          <span class="message-topline"
            ><span class="sender">{message.sender}</span><time
              datetime={new Date(message.receivedAt * 1000).toISOString()}
              >{date(message.receivedAt)}</time
            ></span
          >
          <span class="subject">{message.subject}</span>
          <span class="snippet">{message.snippet}</span>
          <span class="message-labels"
            >{message.labels.join(' · ')}{message.attachmentCount > 0
              ? ` · ${message.attachmentCount} attachment${message.attachmentCount === 1 ? '' : 's'}`
              : ''}</span
          >
        </button>
      </li>
    {/each}
  </ul>
{/if}
