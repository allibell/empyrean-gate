import { useEffect, useRef, useState } from "react";
import { startAudioFeatures } from "./sensors";
import { defaultLayer } from "./types";
import { useGate } from "./state";
import type { ResolvedMedia } from "./ws";

const FRAME_RATES = [10, 15, 24];
const TEXTURE_SIZES = [64, 96, 128];
type AudioMode = "none" | "video" | `source:${number}`;
type MediaKind = "video" | "image";
type ImageMotion = "still" | "ambient" | "breathe" | "drift" | "haze" | "fade";
type ImageFit = "contain" | "cover";

interface LoadedMedia extends ResolvedMedia {
  kind: MediaKind;
}

interface ImageAnimationSettings {
  motion: ImageMotion;
  intensity: number;
  seconds: number;
  fit: ImageFit;
  fadeWhite: boolean;
}

const IMAGE_MOTIONS: ReadonlyArray<{ value: ImageMotion; label: string }> = [
  { value: "ambient", label: "Ambient float" },
  { value: "breathe", label: "Slow breathe" },
  { value: "drift", label: "Orbital drift" },
  { value: "haze", label: "Dream haze" },
  { value: "fade", label: "Arrive & fade" },
  { value: "still", label: "Still" },
];

const BUNDLED_IMAGES = [
  {
    playbackUrl: "/media/entheos.png",
    title: "Entheos",
    label: "Entheos",
    description: "Original camp artwork",
    motion: "ambient" as ImageMotion,
    intensity: 0.55,
    seconds: 60,
    fit: "contain" as ImageFit,
  },
  {
    playbackUrl: "/media/axis-mundi-gate-scene.png",
    title: "Axis Mundi · Full Gate scene",
    label: "Axis Mundi scene",
    description: "Cosmic tree + radial titles",
    motion: "ambient" as ImageMotion,
    intensity: 0.5,
    seconds: 120,
    fit: "contain" as ImageFit,
  },
  {
    playbackUrl: "/media/axis-mundi-tree-gate.png",
    title: "Axis Mundi · Tree emblem",
    label: "Axis Mundi tree",
    description: "Transparent radial tree mark",
    motion: "breathe" as ImageMotion,
    intensity: 0.65,
    seconds: 60,
    fit: "contain" as ImageFit,
  },
] as const;

const clamp01 = (value: number) => Math.max(0, Math.min(1, value));
const smooth = (value: number) => {
  const t = clamp01(value);
  return t * t * (3 - 2 * t);
};

function fadeWhitePixels(ctx: CanvasRenderingContext2D, size: number): void {
  const frame = ctx.getImageData(0, 0, size, size);
  const pixels = frame.data;
  for (let index = 0; index < pixels.length; index += 4) {
    const r = pixels[index];
    const g = pixels[index + 1];
    const b = pixels[index + 2];
    const high = Math.max(r, g, b);
    const low = Math.min(r, g, b);
    const neutral = 1 - clamp01((high - low) / 42);
    const white = smooth((low - 188) / 67) * neutral;
    pixels[index + 3] = Math.round(pixels[index + 3] * (1 - white));
  }
  ctx.putImageData(frame, 0, 0);
}

function imageHasTransparency(image: HTMLImageElement): boolean {
  const sample = document.createElement("canvas");
  sample.width = 64;
  sample.height = 64;
  const ctx = sample.getContext("2d", { willReadFrequently: true });
  if (!ctx) return false;
  ctx.drawImage(image, 0, 0, sample.width, sample.height);
  const pixels = ctx.getImageData(0, 0, sample.width, sample.height).data;
  for (let index = 3; index < pixels.length; index += 4) {
    if (pixels[index] < 250) return true;
  }
  return false;
}

function drawAnimatedImage(
  ctx: CanvasRenderingContext2D,
  image: HTMLImageElement,
  size: number,
  elapsedMs: number,
  settings: ImageAnimationSettings,
): void {
  const cycle = Math.max(5, settings.seconds) * 1000;
  const progress = (elapsedMs % cycle) / cycle;
  const wave = Math.sin(progress * Math.PI * 2);
  const wave2 = Math.sin(progress * Math.PI * 4 + 0.8);
  const amount = settings.intensity;
  let scale = 1;
  let x = 0;
  let y = 0;
  let rotation = 0;
  let opacity = 1;
  let blur = 0;

  switch (settings.motion) {
    case "ambient":
      scale = 1 + amount * (0.035 + wave * 0.018);
      x = size * amount * 0.025 * wave2;
      y = size * amount * 0.018 * Math.cos(progress * Math.PI * 2);
      rotation = amount * 0.018 * wave;
      opacity = 0.92 + wave2 * 0.05;
      blur = amount * (0.4 + (wave + 1) * 0.35);
      break;
    case "breathe":
      scale = 1 + amount * (0.025 + (wave + 1) * 0.025);
      opacity = 0.88 + (wave + 1) * 0.06;
      break;
    case "drift":
      scale = 1 + amount * 0.045;
      x = size * amount * 0.045 * Math.sin(progress * Math.PI * 2);
      y = size * amount * 0.035 * Math.sin(progress * Math.PI * 4 + 1.2);
      rotation = amount * 0.035 * wave2;
      break;
    case "haze":
      scale = 1 + amount * (0.035 + (wave + 1) * 0.018);
      opacity = 0.82 + (wave2 + 1) * 0.08;
      blur = amount * (0.7 + (wave + 1) * 1.1);
      break;
    case "fade": {
      scale = 0.97 + amount * 0.1 * smooth(progress);
      const envelope = progress < 0.14
        ? smooth(progress / 0.14)
        : progress < 0.7
          ? 1
          : 1 - smooth((progress - 0.7) / 0.3);
      opacity = envelope;
      rotation = amount * 0.018 * wave;
      blur = amount * (1 - envelope) * 2;
      break;
    }
    case "still":
      break;
  }

  const naturalWidth = image.naturalWidth;
  const naturalHeight = image.naturalHeight;
  const baseScale = settings.fit === "cover"
    ? Math.max(size / naturalWidth, size / naturalHeight)
    : Math.min(size / naturalWidth, size / naturalHeight);
  const width = naturalWidth * baseScale * scale;
  const height = naturalHeight * baseScale * scale;

  ctx.clearRect(0, 0, size, size);
  ctx.save();
  ctx.translate(size / 2 + x, size / 2 + y);
  ctx.rotate(rotation);
  ctx.globalAlpha = clamp01(opacity);
  ctx.filter = blur > 0.05 ? `blur(${blur.toFixed(2)}px)` : "none";
  ctx.drawImage(image, -width / 2, -height / 2, width, height);
  ctx.restore();
  ctx.filter = "none";
  if (settings.fadeWhite) fadeWhitePixels(ctx, size);
}

interface SoundtrackGraph {
  element: HTMLVideoElement;
  context: AudioContext;
  source: MediaElementAudioSourceNode;
  analyser: AnalyserNode;
  output: GainNode;
  stopFeatures: (() => void) | null;
}

export default function Media() {
  const { client, config, connected, status } = useGate();
  const [url, setUrl] = useState("");
  const [media, setMedia] = useState<LoadedMedia | null>(null);
  const [resolving, setResolving] = useState(false);
  const [broadcasting, setBroadcasting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [transportFps, setTransportFps] = useState(15);
  const [textureSize, setTextureSize] = useState(96);
  const [audioMode, setAudioMode] = useState<AudioMode>("video");
  const [audioAmount, setAudioAmount] = useState(0.7);
  const [monitorSoundtrack, setMonitorSoundtrack] = useState(false);
  const [imageMotion, setImageMotion] = useState<ImageMotion>("ambient");
  const [imageMotionAmount, setImageMotionAmount] = useState(0.65);
  const [imageCycleSeconds, setImageCycleSeconds] = useState(60);
  const [imageFit, setImageFit] = useState<ImageFit>("contain");
  const [fadeWhite, setFadeWhite] = useState(true);
  const [sent, setSent] = useState(0);
  const [dropped, setDropped] = useState(0);
  const videoRef = useRef<HTMLVideoElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const localObjectUrl = useRef<string | null>(null);
  const claimStartedAt = useRef(0);
  const soundtrackRef = useRef<SoundtrackGraph | null>(null);
  const imageAnimationRef = useRef<ImageAnimationSettings>({
    motion: "ambient",
    intensity: 0.65,
    seconds: 60,
    fit: "contain",
    fadeWhite: true,
  });
  imageAnimationRef.current = {
    motion: imageMotion,
    intensity: imageMotionAmount,
    seconds: imageCycleSeconds,
    fit: imageFit,
    fadeWhite,
  };

  const pauseSoundtrack = () => {
    const graph = soundtrackRef.current;
    if (!graph) return;
    graph.stopFeatures?.();
    graph.stopFeatures = null;
    graph.output.gain.value = 0;
  };

  const destroySoundtrack = () => {
    const graph = soundtrackRef.current;
    if (!graph) return;
    pauseSoundtrack();
    graph.source.disconnect();
    graph.analyser.disconnect();
    graph.output.disconnect();
    void graph.context.close();
    soundtrackRef.current = null;
  };

  const startSoundtrack = (video: HTMLVideoElement) => {
    let graph = soundtrackRef.current;
    if (!graph || graph.element !== video) {
      destroySoundtrack();
      const context = new AudioContext();
      const source = context.createMediaElementSource(video);
      const analyser = context.createAnalyser();
      const output = context.createGain();
      source.connect(analyser);
      analyser.connect(output);
      output.connect(context.destination);
      graph = { element: video, context, source, analyser, output, stopFeatures: null };
      soundtrackRef.current = graph;
    }
    // createMediaElementSource reroutes playback through this graph. Keep the
    // element itself unmuted so the analyser receives samples; the output gain
    // independently controls whether this iPad/laptop is audible.
    video.muted = false;
    graph.output.gain.value = monitorSoundtrack ? 1 : 0;
    graph.stopFeatures ??= startAudioFeatures(client, graph.context, graph.analyser, "video");
    void graph.context.resume();
  };

  const configureVideoReaction = (mode: AudioMode, amount: number): boolean => {
    if (!config) return false;
    const sources = [...config.audio.sources];
    let sourceIndex = 0;
    if (mode === "video") {
      sourceIndex = sources.findIndex((source) => source.kind === "video");
      if (sourceIndex < 0) {
        if (sources.length >= 4) {
          setError("All four audio-source slots are in use. Remove one in Settings to analyze the video soundtrack.");
          return false;
        }
        sourceIndex = sources.length;
        sources.push({ id: "video", kind: "video", gain: 1 });
      }
    } else if (mode.startsWith("source:")) {
      sourceIndex = Number(mode.slice(7));
      if (!sources[sourceIndex] || sources[sourceIndex].kind === "video") {
        setError("That live audio source is no longer available. Choose another source.");
        return false;
      }
    }

    const layers = config.layers.map((layer) =>
      layer.kind === "video"
        ? { ...layer, audio_source: sourceIndex, audio_amount: mode === "none" ? 0 : amount }
        : layer,
    );
    if (!layers.some((layer) => layer.kind === "video")) {
      layers.push({
        ...defaultLayer("video"),
        audio_source: sourceIndex,
        audio_amount: mode === "none" ? 0 : amount,
      });
    }
    client.setConfig({ ...config, audio: { sources }, layers });
    return true;
  };

  const replaceMedia = (next: LoadedMedia) => {
    destroySoundtrack();
    if (localObjectUrl.current && localObjectUrl.current !== next.playbackUrl) {
      URL.revokeObjectURL(localObjectUrl.current);
    }
    localObjectUrl.current = next.playbackUrl.startsWith("blob:") ? next.playbackUrl : null;
    setBroadcasting(false);
    setSent(0);
    setDropped(0);
    setError(null);
    setMedia(next);
    if (next.kind === "image") {
      setAudioMode("none");
      setFadeWhite(true);
    } else {
      setAudioMode("video");
    }
  };

  const resolveUrl = async () => {
    if (!url.trim() || !connected) return;
    setResolving(true);
    setError(null);
    try {
      replaceMedia({ ...await client.resolveMedia(url.trim()), kind: "video" });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setResolving(false);
    }
  };

  const loadFile = (file: File | undefined) => {
    if (!file) return;
    const isImage = file.type.startsWith("image/") || /\.(avif|gif|heic|jpe?g|png|webp)$/i.test(file.name);
    const isVideo = file.type.startsWith("video/") || /\.(m4v|mov|mp4|webm)$/i.test(file.name);
    if (!isImage && !isVideo) {
      setError("Choose an image or video file that this browser can display.");
      return;
    }
    const playbackUrl = URL.createObjectURL(file);
    replaceMedia({
      playbackUrl,
      title: file.name,
      sourceUrl: `local file: ${file.name}`,
      resolvedBy: "this device",
      kind: isImage ? "image" : "video",
    });
  };

  const loadBundledImage = (image: (typeof BUNDLED_IMAGES)[number]) => {
    replaceMedia({
      playbackUrl: image.playbackUrl,
      title: image.title,
      sourceUrl: `bundled artwork: ${image.playbackUrl}`,
      resolvedBy: "Gate artwork",
      kind: "image",
    });
    setFadeWhite(image.playbackUrl === "/media/entheos.png");
    setImageMotion(image.motion);
    setImageMotionAmount(image.intensity);
    setImageCycleSeconds(image.seconds);
    setImageFit(image.fit);
    setTextureSize(128);
    setTransportFps(15);
  };

  const goLive = () => {
    const video = videoRef.current;
    const image = imageRef.current;
    if (!media || (media.kind === "video" && !video)) return;
    if (media.kind === "image" && (!image || !image.complete || image.naturalWidth === 0)) {
      setError("The image is still loading. Try again in a moment.");
      return;
    }
    if (!configureVideoReaction(audioMode, audioAmount)) return;
    if (media.kind === "video" && audioMode === "video") {
      if (!video) return;
      try {
        startSoundtrack(video);
      } catch (e) {
        setError(`The soundtrack could not be analyzed: ${e instanceof Error ? e.message : e}`);
        return;
      }
    } else {
      pauseSoundtrack();
    }
    claimStartedAt.current = performance.now();
    client.startVideo(media.title, media.sourceUrl);
    if (media.kind === "image") {
      pauseSoundtrack();
      setBroadcasting(true);
      return;
    }
    if (!video) return;
    // Calling play synchronously from this tap matters on iPadOS.
    void video
      .play()
      .then(() => {
        setBroadcasting(true);
      })
      .catch((e) => {
        pauseSoundtrack();
        client.stopVideo();
        setError(`Playback could not start: ${e instanceof Error ? e.message : e}`);
      });
  };

  const stop = () => {
    setBroadcasting(false);
    pauseSoundtrack();
    client.stopVideo();
  };

  useEffect(() => {
    if (!broadcasting || !media || !connected) return;
    const video = videoRef.current;
    const image = imageRef.current;
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d", { willReadFrequently: true });
    if (!canvas || !ctx || (media.kind === "video" ? !video : !image)) return;
    claimStartedAt.current = performance.now();
    client.startVideo(media.title, media.sourceUrl);
    canvas.width = textureSize;
    canvas.height = textureSize;
    let cancelled = false;
    let callbackId = 0;
    let timer = 0;
    let lastSent = 0;
    const interval = 1000 / transportFps;

    const capture = (now: number) => {
      if (cancelled) return;
      if (now - lastSent >= interval) {
        if (media.kind === "image" && image && image.complete && image.naturalWidth > 0) {
          try {
            drawAnimatedImage(
              ctx,
              image,
              textureSize,
              now - claimStartedAt.current,
              imageAnimationRef.current,
            );
            const rgba = ctx.getImageData(0, 0, textureSize, textureSize).data;
            if (client.sendVideoFrame(textureSize, textureSize, rgba)) {
              setSent((n) => n + 1);
            } else {
              setDropped((n) => n + 1);
            }
            lastSent = now;
          } catch {
            setError("The browser could not animate this image. Try a PNG, JPEG, or WebP file.");
            setBroadcasting(false);
            client.stopVideo();
            return;
          }
        } else if (media.kind === "video" && video && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA) {
          const sw = video.videoWidth;
          const sh = video.videoHeight;
          if (sw > 0 && sh > 0) {
          // Center-crop to a square before the GPU's radial/kaleidoscope mapping.
          const side = Math.min(sw, sh);
          const sx = (sw - side) / 2;
          const sy = (sh - side) / 2;
          try {
            ctx.drawImage(video, sx, sy, side, side, 0, 0, textureSize, textureSize);
            const rgba = ctx.getImageData(0, 0, textureSize, textureSize).data;
            if (client.sendVideoFrame(textureSize, textureSize, rgba)) {
              setSent((n) => n + 1);
            } else {
              setDropped((n) => n + 1);
            }
            lastSent = now;
          } catch {
            setError("The browser blocked access to this video's pixels. Reload it through the Gate URL resolver.");
            setBroadcasting(false);
            client.stopVideo();
            return;
          }
          }
        }
      }
      schedule();
    };

    const schedule = () => {
      if (media.kind === "video" && video && "requestVideoFrameCallback" in video) {
        callbackId = video.requestVideoFrameCallback((now) => capture(now));
      } else {
        timer = window.setTimeout(() => capture(performance.now()), interval);
      }
    };
    schedule();
    return () => {
      cancelled = true;
      if (callbackId && video && "cancelVideoFrameCallback" in video) {
        video.cancelVideoFrameCallback(callbackId);
      }
      clearTimeout(timer);
      client.stopVideo();
    };
  }, [broadcasting, client, connected, media, textureSize, transportFps]);

  // If another device takes over, stop doing local capture once the status
  // round-trip confirms it. Cleanup is owner-scoped, so it cannot stop the winner.
  useEffect(() => {
    if (
      broadcasting &&
      status?.video.active &&
      status.video.owner_id !== client.clientId &&
      performance.now() - claimStartedAt.current > 1500
    ) {
      setBroadcasting(false);
    }
  }, [broadcasting, client.clientId, status]);

  useEffect(
    () => () => {
      destroySoundtrack();
      if (localObjectUrl.current) URL.revokeObjectURL(localObjectUrl.current);
    },
    [],
  );

  useEffect(() => {
    const graph = soundtrackRef.current;
    if (graph) graph.output.gain.value = monitorSoundtrack && broadcasting && audioMode === "video" ? 1 : 0;
  }, [audioMode, broadcasting, monitorSoundtrack]);

  const active = status?.video;
  const ownedHere = active?.active && active.owner_id === client.clientId;
  const soundtrackIndex = config?.audio.sources.findIndex((source) => source.kind === "video") ?? -1;
  const soundtrackStatus = soundtrackIndex >= 0 ? status?.audio[soundtrackIndex] : undefined;

  const changeAudioMode = (next: AudioMode) => {
    setAudioMode(next);
    if (!broadcasting) return;
    if (!configureVideoReaction(next, audioAmount)) return;
    if (next === "video" && media?.kind === "video" && videoRef.current) {
      try {
        startSoundtrack(videoRef.current);
      } catch (e) {
        setError(`The soundtrack could not be analyzed: ${e instanceof Error ? e.message : e}`);
      }
    } else {
      pauseSoundtrack();
    }
  };

  return (
    <div className="media-page">
      <section className="panel media-source-panel">
        <div className="media-heading">
          <div>
            <h2>Image or video source</h2>
            <p className="hint">
              Bring in a still image from this device or paste a video URL. Still images can float,
              breathe, haze, or fade for ambient and long-play scenes; only a tiny live texture
              crosses the show LAN.
            </p>
          </div>
          {active?.active && (
            <span className="media-live-pill">
              LIVE · {active.width}×{active.height} · {active.fps.toFixed(1)} fps
            </span>
          )}
        </div>

        <form
          className="media-url-row"
          onSubmit={(e) => {
            e.preventDefault();
            void resolveUrl();
          }}
        >
          <input
            type="url"
            inputMode="url"
            placeholder="https://… video or page URL"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <button type="submit" disabled={!connected || resolving || !url.trim()}>
            {resolving ? "Finding video…" : "Load URL"}
          </button>
        </form>
        <div className="media-or"><span>or</span></div>
        <div className="media-file-buttons">
          <label className="media-file-button media-file-button-primary">
            <strong>Choose an image</strong>
            <span>PNG, JPEG, WebP, GIF, or AVIF</span>
            <input type="file" accept="image/*" onChange={(e) => loadFile(e.target.files?.[0])} />
          </label>
          <label className="media-file-button">
            <strong>Choose a video</strong>
            <span>MP4, MOV, or WebM</span>
            <input type="file" accept="video/*" onChange={(e) => loadFile(e.target.files?.[0])} />
          </label>
        </div>
        <div className="media-bundled-images" aria-label="Saved media scenes">
          <span>Saved media scenes</span>
          {BUNDLED_IMAGES.map((image) => (
            <button
              type="button"
              key={image.playbackUrl}
              className={media?.playbackUrl === image.playbackUrl ? "active" : undefined}
              aria-pressed={media?.playbackUrl === image.playbackUrl}
              onClick={() => loadBundledImage(image)}
            >
              <strong>{image.label}</strong>
              <small>{image.description}</small>
            </button>
          ))}
        </div>
        {error && <p className="media-error">{error}</p>}
      </section>

      {media && (
        <section className="panel media-player-panel">
          <div className="media-stage">
            {media.kind === "video" ? (
              <video
                ref={videoRef}
                key={media.playbackUrl}
                src={media.playbackUrl}
                controls
                playsInline
                muted={audioMode !== "video" || !broadcasting}
                loop
                preload="metadata"
                crossOrigin="anonymous"
              />
            ) : (
              <img
                ref={imageRef}
                key={media.playbackUrl}
                src={media.playbackUrl}
                alt={media.title}
                onLoad={(event) => {
                  setError(null);
                  setFadeWhite(
                    media.playbackUrl === "/media/entheos.png"
                      ? true
                      : media.resolvedBy === "Gate artwork"
                        ? false
                        : !imageHasTransparency(event.currentTarget),
                  );
                }}
                onError={() => setError("This browser could not decode that image. Try PNG, JPEG, or WebP.")}
              />
            )}
            <canvas ref={canvasRef} className="media-texture-preview" aria-label="Texture sent to the Gate" />
          </div>
          <div className="media-info">
            <div>
              <strong>{media.title}</strong>
              <span>resolved by {media.resolvedBy}</span>
            </div>
            <div className="media-transport-controls">
              <label>
                Texture
                <select value={textureSize} onChange={(e) => setTextureSize(Number(e.target.value))}>
                  {TEXTURE_SIZES.map((n) => <option key={n} value={n}>{n}×{n}</option>)}
                </select>
              </label>
              <label>
                Send rate
                <select value={transportFps} onChange={(e) => setTransportFps(Number(e.target.value))}>
                  {FRAME_RATES.map((n) => <option key={n} value={n}>{n} fps</option>)}
                </select>
              </label>
            </div>
          </div>
          {media.kind === "image" && (
            <div className="media-image-controls">
              <label>
                Motion
                <select value={imageMotion} onChange={(e) => setImageMotion(e.target.value as ImageMotion)}>
                  {IMAGE_MOTIONS.map((motion) => (
                    <option key={motion.value} value={motion.value}>{motion.label}</option>
                  ))}
                </select>
              </label>
              <label className="media-motion-amount">
                Amount <strong>{Math.round(imageMotionAmount * 100)}%</strong>
                <input
                  type="range"
                  min="0"
                  max="1"
                  step="0.05"
                  value={imageMotionAmount}
                  disabled={imageMotion === "still"}
                  onChange={(e) => setImageMotionAmount(Number(e.target.value))}
                />
              </label>
              <label>
                Loop
                <select value={imageCycleSeconds} onChange={(e) => setImageCycleSeconds(Number(e.target.value))}>
                  <option value={20}>20 sec</option>
                  <option value={60}>1 min</option>
                  <option value={120}>2 min</option>
                  <option value={300}>5 min</option>
                </select>
              </label>
              <label>
                Framing
                <select value={imageFit} onChange={(e) => setImageFit(e.target.value as ImageFit)}>
                  <option value="contain">Show all</option>
                  <option value="cover">Fill frame</option>
                </select>
              </label>
              <label className="media-white-toggle">
                <input type="checkbox" checked={fadeWhite} onChange={(e) => setFadeWhite(e.target.checked)} />
                Fade white background
              </label>
            </div>
          )}
          <div className="media-audio-controls">
            <label>
              Rhythm source
              <select value={audioMode} onChange={(e) => changeAudioMode(e.target.value as AudioMode)}>
                {media.kind === "video" && <option value="video">Video soundtrack</option>}
                {(config?.audio.sources ?? []).some((source) => source.kind !== "video") && (
                  <optgroup label="Gate live inputs">
                    {(config?.audio.sources ?? []).map((source, index) =>
                      source.kind !== "video" ? (
                        <option key={`${source.id}-${index}`} value={`source:${index}`}>
                          {source.id}
                        </option>
                      ) : null,
                    )}
                  </optgroup>
                )}
                <option value="none">Visual only</option>
              </select>
            </label>
            <label className="media-audio-amount">
              Response <strong>{Math.round(audioAmount * 100)}%</strong>
              <input
                type="range"
                min="0"
                max="1.5"
                step="0.05"
                value={audioAmount}
                disabled={audioMode === "none"}
                onChange={(e) => {
                  const next = Number(e.target.value);
                  setAudioAmount(next);
                  if (broadcasting) configureVideoReaction(audioMode, next);
                }}
              />
            </label>
            {media.kind === "video" && audioMode === "video" && (
              <label className="media-monitor-toggle">
                <input
                  type="checkbox"
                  checked={monitorSoundtrack}
                  onChange={(e) => setMonitorSoundtrack(e.target.checked)}
                />
                Hear soundtrack here
              </label>
            )}
            {media.kind === "video" && audioMode === "video" && broadcasting && (
              <span className={soundtrackStatus?.active ? "ok" : "hint"}>
                {soundtrackStatus?.active
                  ? `Soundtrack live${soundtrackStatus.bpm > 0 ? ` · ${soundtrackStatus.bpm.toFixed(0)} BPM` : " · finding beat…"}`
                  : "Starting soundtrack analysis…"}
              </span>
            )}
          </div>
          <div className="media-actions">
            {!broadcasting ? (
              <button className="primary" onClick={goLive} disabled={!connected}>
                Play {media.kind} on Gate
              </button>
            ) : (
              <button className="danger" onClick={stop}>Stop Gate {media.kind}</button>
            )}
            <span className="hint">
              {broadcasting
                ? `${sent.toLocaleString()} frames sent${dropped ? ` · ${dropped} dropped to stay live` : ""}`
                : `This ${media.kind} stays local until you play it on the Gate.`}
            </span>
          </div>
        </section>
      )}

      <section className="panel media-treatment-panel">
        <h2>Gate treatment</h2>
        {active?.active ? (
          <p>
            <strong>{active.title || "Untitled visual"}</strong> is coming from {active.owner_name || "a connected device"}.
            {ownedHere ? " This device owns the live feed." : " Starting another source will take it over cleanly."}
          </p>
        ) : (
          <p className="hint">No media frames are live. The last frame is removed immediately when its source stops or disconnects.</p>
        )}
        <p className="hint">
          Add or edit a <strong>Video</strong> layer in Settings to shape it: Zoom, Kaleidoscope,
          Contrast, Rotation, saturation, tint/original-color mix, brightness, blend, opacity,
          speed, and audio response all remain live and composable with the other patterns. The
          rhythm source above can be the video's own soundtrack or any configured Gate input. For
          still images, the animation controls keep the motion slow and intentional before the
          layer stack adds its own treatment.
        </p>
        {active?.active && !broadcasting && (
          <button className="danger" onClick={() => client.stopVideo(true)}>Stop current source</button>
        )}
      </section>
    </div>
  );
}
