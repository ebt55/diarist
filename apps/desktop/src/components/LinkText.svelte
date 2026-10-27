<script lang="ts">
  // A sentence with its web addresses selectable and a Copy link button after
  // each (a checklist row names the Microsoft Visual C++ download page). The
  // app opens no browser by itself; the user pastes the address.
  import { errorText } from "../lib/api";
  import { linkParts } from "../lib/links";
  import { app } from "../lib/state.svelte";

  interface Props {
    text: string;
  }
  let { text }: Props = $props();

  const parts = $derived(linkParts(text));

  async function copy(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      app.toast("info", "Link copied; paste it into a browser.");
    } catch (e) {
      app.error(`Could not copy the link (${errorText(e)}); select the address and copy it by hand`);
    }
  }
</script>

{#each parts as p, i (i)}{#if p.url}<code class="sel">{p.text}</code>
    <button type="button" class="link" onclick={() => copy(p.text)} aria-label="Copy link {p.text}">Copy link</button
    >{:else}{p.text}{/if}{/each}

<style>
  .sel {
    user-select: all;
    overflow-wrap: anywhere;
  }
</style>
