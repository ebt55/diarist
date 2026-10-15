<script lang="ts" module>
  // 16 px stroke icons (paths drawn for a 24 px grid).
  const PATHS = {
    import: "M12 3v12m0 0-4-4m4 4 4-4M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2",
    settings:
      "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm7.4-3a7.4 7.4 0 0 0-.1-1.2l2-1.6-2-3.4-2.4 1a7.6 7.6 0 0 0-2-1.2L14.5 3h-4l-.4 2.6a7.6 7.6 0 0 0-2 1.2l-2.4-1-2 3.4 2 1.6a7.4 7.4 0 0 0 0 2.4l-2 1.6 2 3.4 2.4-1a7.6 7.6 0 0 0 2 1.2l.4 2.6h4l.4-2.6a7.6 7.6 0 0 0 2-1.2l2.4 1 2-3.4-2-1.6c.1-.4.1-.8.1-1.2Z",
    log: "M5 4h14v16H5zM8 8h8M8 12h8M8 16h5",
    search: "m20 20-4.5-4.5M17 10.5a6.5 6.5 0 1 1-13 0 6.5 6.5 0 0 1 13 0Z",
    play: "M7 4.5v15l12-7.5-12-7.5Z",
    pause: "M7 4h3.5v16H7zM13.5 4H17v16h-3.5z",
    back: "M11 18 5 12l6-6M5 12h14",
    more: "M5 12h.01M12 12h.01M19 12h.01",
    pin: "M9 3h6l-1 6 4 4H6l4-4-1-6Zm3 10v8",
    export: "M12 15V3m0 0L8 7m4-4 4 4M4 13v6a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-6",
    trash: "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
    folder: "M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6Z",
    refresh: "M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5",
    copy: "M9 9h11v11H9zM5 15H4V4h11v1",
    close: "M6 6l12 12M18 6 6 18",
    keyboard: "M3 7h18v10H3zM7 11h.01M11 11h.01M15 11h.01M8 14h8",
    download: "M12 4v11m0 0-4-4m4 4 4-4M5 20h14",
  } as const;
  export type IconName = keyof typeof PATHS;
</script>

<script lang="ts">
  interface Props {
    name: IconName;
    size?: number;
  }
  let { name, size = 16 }: Props = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={name === "play" || name === "pause" ? "currentColor" : "none"}
  stroke="currentColor"
  stroke-width={name === "more" ? 3.2 : 1.8}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  <path d={PATHS[name]} />
</svg>
