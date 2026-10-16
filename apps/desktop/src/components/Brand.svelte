<script lang="ts">
  // The sidebar's brand: the word "Diarist", or the app icon when the
  // header has no room for the word (the recording timer widens the Record
  // button). The icon follows the UI theme the way app.css does: icon.svg
  // (the ink page) on the light UI, icon-dark.svg (the paper page) on the
  // dark one. Nothing here takes the focus or a key.
  import { untrack } from "svelte";
  import iconLight from "../../../../branding/icon.svg";
  import iconDark from "../../../../branding/icon-dark.svg";

  let brand: HTMLElement | undefined = $state();
  let word: HTMLElement | undefined = $state();
  let compact = $state(false);
  /** The word's natural width, measured while it is shown. */
  let wordWidth = 0;

  $effect(() => {
    if (!brand || !word) return;
    const [b, w] = [brand, word];
    // The brand fills what the buttons leave, whatever it shows, so its
    // width alone decides (no back and forth between the two states).
    const fit = () => {
      if (!compact) wordWidth = Math.max(wordWidth, w.scrollWidth);
      const next = wordWidth > b.clientWidth + 0.5;
      if (next !== compact) compact = next;
    };
    const ro = new ResizeObserver(() => fit());
    ro.observe(b);
    untrack(fit);
    return () => ro.disconnect();
  });
</script>

<span class="brand" class:compact bind:this={brand} title={compact ? "Diarist" : undefined}>
  <img class="icon light" src={iconLight} alt="" width="20" height="20" />
  <img class="icon dark" src={iconDark} alt="" width="20" height="20" />
  <span class="word" class:sr-only={compact} bind:this={word}>Diarist</span>
</span>

<style>
  .brand {
    display: flex;
    flex: 1 1 auto;
    align-items: center;
    /* The same box whichever it shows, so a switch never resizes what the
       ResizeObserver watches (no loop). */
    height: 20px;
    min-width: 0;
    overflow: hidden;
    font-weight: 700;
    letter-spacing: -0.01em;
    white-space: nowrap;
  }
  .word {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .icon {
    display: none;
    flex: none;
    width: 20px;
    height: 20px;
  }
  .compact .icon.light {
    display: block;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root:not([data-theme="light"])) .compact .icon.light {
      display: none;
    }
    :global(:root:not([data-theme="light"])) .compact .icon.dark {
      display: block;
    }
  }
  :global(:root[data-theme="dark"]) .compact .icon.light {
    display: none;
  }
  :global(:root[data-theme="dark"]) .compact .icon.dark {
    display: block;
  }
</style>
