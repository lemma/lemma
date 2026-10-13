package com.lemmabase.lemma;

import java.lang.ref.Cleaner;
import java.net.http.HttpClient;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.locks.ReentrantLock;
import org.jspecify.annotations.Nullable;

/** Lemma rules engine. Thread-safe. All methods acquire an internal lock; concurrent calls from multiple threads are safe. */
public final class Engine implements AutoCloseable {
  /** Cleaner for native handle teardown when {@link Engine} is unreachable. */
  private static final Cleaner CLEANER = Cleaner.create();

  /** Shared HTTP client for LemmaBase requests (JVM defaults for proxy and trust store). */
  private static final HttpClient DEFAULT_HTTP_CLIENT =
      HttpClient.newBuilder().followRedirects(HttpClient.Redirect.NORMAL).build();

  /** Shared native state guarded by {@link NativeState#lock}. */
  private final NativeState state;
  /** Deregisters {@link DestroyAction} after explicit {@link #close()}. */
  private final Cleaner.Cleanable cleanable;

  /**
   * Creates an engine from an already-allocated native handle.
   *
   * @param handle native engine handle from JNI
   */
  private Engine(long handle) {
    if (handle == 0L) {
      throw new LemmaBugError("BUG: native engine create returned null handle");
    }
    this.state = new NativeState(handle);
    this.cleanable = CLEANER.register(this, new DestroyAction(state));
  }

  /**
   * Returns the native engine version (e.g. {@code "0.9.12"}).
   *
   * @return semver string
   */
  public static String version() {
    return Native.version();
  }

  /**
   * Creates an engine with default resource limits.
   *
   * @return new engine instance
   */
  public static Engine create() {
    return new Engine(Native.create());
  }

  /**
   * Creates an engine with explicit resource limits.
   *
   * @param limits limits applied at creation
   * @return new engine instance
   */
  public static Engine create(ResourceLimits limits) {
    Objects.requireNonNull(limits, "limits");
    return new Engine(Native.createWithLimits(JsonSupport.limitsToJson(limits)));
  }

  /**
   * Creates an engine with named resource-limit overrides. Unset keys keep engine defaults.
   *
   * @param limits named overrides
   * @return new engine instance
   */
  public static Engine create(ResourceLimits.Builder limits) {
    Objects.requireNonNull(limits, "limits");
    return new Engine(Native.createWithLimits(limits.toJson()));
  }

  /**
   * Restores an engine from {@link #snapshot()} bytes.
   *
   * @param bytes opaque snapshot from {@link #snapshot()}
   * @return restored engine
   */
  public static Engine fromSnapshot(byte[] bytes) {
    Objects.requireNonNull(bytes, "bytes");
    return new Engine(Native.fromSnapshot(bytes));
  }

  /**
   * Loads Lemma source into the engine.
   *
   * @param code Lemma source text
   */
  public void load(String code) {
    Objects.requireNonNull(code, "code");
    state.lock.lock();
    try {
      Native.load(state.requireHandle(), code);
    } finally {
      state.lock.unlock();
    }
  }

  /**
   * Loads labeled sources (label to source text).
   *
   * @param sources map from source label to Lemma text
   */
  public void load(Map<String, String> sources) {
    Objects.requireNonNull(sources, "sources");
    String[] labels = sources.keySet().toArray(String[]::new);
    String[] codes = new String[labels.length];
    for (int i = 0; i < labels.length; i++) {
      codes[i] = Objects.requireNonNull(sources.get(labels[i]), "source code");
    }
    state.lock.lock();
    try {
      Native.loadLabeled(state.requireHandle(), labels, codes);
    } finally {
      state.lock.unlock();
    }
  }

  /**
   * Loads UTF-8 Lemma source from a file path.
   *
   * @param path source file
   * @throws LemmaException if the file cannot be read
   */
  public void load(java.nio.file.Path path) {
    Objects.requireNonNull(path, "path");
    String code;
    try {
      code = java.nio.file.Files.readString(path, java.nio.charset.StandardCharsets.UTF_8);
    } catch (java.io.IOException e) {
      String message = "failed to read Lemma source from '" + path + "': " + e.getMessage();
      throw new LemmaException(message, List.of(EngineError.request(message)));
    }
    load(code);
  }

  /**
   * Loads UTF-8 Lemma source from a classpath resource.
   *
   * @param anchor class whose class loader locates the resource
   * @param resourcePath classpath resource path
   * @throws LemmaException if the resource is missing or cannot be read
   */
  public void loadResource(Class<?> anchor, String resourcePath) {
    Objects.requireNonNull(anchor, "anchor");
    Objects.requireNonNull(resourcePath, "resourcePath");
    try (java.io.InputStream in = anchor.getClassLoader().getResourceAsStream(resourcePath)) {
      if (in == null) {
        String message = "classpath resource not found: '" + resourcePath + "'";
        throw new LemmaException(message, List.of(EngineError.request(message)));
      }
      load(new String(in.readAllBytes(), java.nio.charset.StandardCharsets.UTF_8));
    } catch (java.io.IOException e) {
      String message =
          "failed to read classpath resource '" + resourcePath + "': " + e.getMessage();
      throw new LemmaException(message, List.of(EngineError.request(message)));
    }
  }

  /**
   * Downloads a repository from LemmaBase; does not load it. Call {@link #load(Map)} with the
   * returned id as the source label before loading workspace specs that {@code uses} it.
   *
   * <p>Does not write {@code lemma_deps/}.
   *
   * @param repository LemmaBase repository id (e.g. {@code @iso/countries})
   * @return downloaded source and repository id
   */
  public RepositoryInstallResult install(String repository) {
    return install(repository, DEFAULT_HTTP_CLIENT);
  }

  /**
   * Downloads a repository from LemmaBase using a caller-configured HTTP client; does not load it.
   * Call {@link #load(Map)} with the returned id as the source label before loading workspace specs
   * that {@code uses} it.
   *
   * @param repository LemmaBase repository id (e.g. {@code @iso/countries})
   * @param client HTTP client (honours JVM trust store and proxy settings when built from defaults)
   * @return downloaded source and repository id
   */
  public RepositoryInstallResult install(String repository, HttpClient client) {
    Objects.requireNonNull(repository, "repository");
    Objects.requireNonNull(client, "client");
    String limitsJson;
    state.lock.lock();
    try {
      limitsJson = Native.limits(state.requireHandle());
    } finally {
      state.lock.unlock();
    }
    return LemmaBase.install(client, repository, limitsJson);
  }

  /**
   * Specs currently loaded in the engine, grouped by repository.
   *
   * @return loaded repositories and their spec versions
   */
  public List<Repository> specs() {
    String json;
    state.lock.lock();
    try {
      json = Native.specs(state.requireHandle());
    } finally {
      state.lock.unlock();
    }
    return JsonSupport.parseRepositories(json);
  }

  /**
   * Returns structured show metadata for one spec slice.
   *
   * @param repository repository handle; {@code null} for default
   * @param spec spec name
   * @param effective effective date; {@code null} for latest
   * @return show metadata
   */
  public Show show(@Nullable String repository, String spec, @Nullable String effective) {
    Objects.requireNonNull(spec, "spec");
    String json;
    state.lock.lock();
    try {
      json = Native.show(state.requireHandle(), repository, spec, effective);
    } finally {
      state.lock.unlock();
    }
    return JsonSupport.parseShow(json);
  }

  /**
   * Returns raw Lemma source for one spec slice.
   *
   * @param repository repository handle; {@code null} for default
   * @param spec spec name; {@code null} when returning whole repository source
   * @param effective effective date; {@code null} for latest
   * @return Lemma source text
   */
  public String source(
      @Nullable String repository, @Nullable String spec, @Nullable String effective) {
    state.lock.lock();
    try {
      return Native.source(state.requireHandle(), repository, spec, effective);
    } finally {
      state.lock.unlock();
    }
  }

  /**
   * Evaluates rules for one spec slice.
   *
   * @param request evaluate parameters and input data
   * @return evaluation response
   */
  public Response evaluate(EvaluateRequest request) {
    Objects.requireNonNull(request, "request");
    String[] rules =
        request.rules() == null ? null : request.rules().toArray(String[]::new);
    String json;
    state.lock.lock();
    try {
      json =
          Native.run(
              state.requireHandle(),
              request.repository(),
              request.spec(),
              request.effective(),
              request.data(),
              rules,
              request.explain());
    } finally {
      state.lock.unlock();
    }
    return JsonSupport.parseResponse(json);
  }

  /**
   * Deprecated alias of {@link #evaluate(EvaluateRequest)}.
   *
   * @param request evaluate parameters and input data
   * @return evaluation response
   * @deprecated Use {@link #evaluate(EvaluateRequest)}
   */
  @Deprecated
  public Response run(EvaluateRequest request) {
    return evaluate(request);
  }

  /**
   * Removes one loaded spec slice.
   *
   * @param repository repository handle; {@code null} for default
   * @param spec spec name
   * @param effective effective date; {@code null} for latest
   */
  public void remove(@Nullable String repository, String spec, @Nullable String effective) {
    Objects.requireNonNull(spec, "spec");
    state.lock.lock();
    try {
      Native.remove(state.requireHandle(), repository, spec, effective);
    } finally {
      state.lock.unlock();
    }
  }

  /**
   * Replace identities in {@code code} (atomic upsert; Path/Dependency prune siblings).
   *
   * @param repository repository handle; {@code null} for default
   * @param code replacement Lemma source
   * @param attribute source label (path or {@code @owner/repo}); {@code null} for volatile
   */
  public void update(
      @Nullable String repository, String code, @Nullable String attribute) {
    Objects.requireNonNull(code, "code");
    state.lock.lock();
    try {
      Native.update(state.requireHandle(), repository, code, attribute);
    } finally {
      state.lock.unlock();
    }
  }

  /** Returns active resource limits.
   *
   * @return current limits
   */
  public ResourceLimits limits() {
    String json;
    state.lock.lock();
    try {
      json = Native.limits(state.requireHandle());
    } finally {
      state.lock.unlock();
    }
    return JsonSupport.parseLimits(json);
  }

  /**
   * Persist parsed specs + plans + limits as opaque bytes. Restore with {@link #fromSnapshot}.
   *
   * @return snapshot bytes
   */
  public byte[] snapshot() {
    state.lock.lock();
    try {
      return Native.snapshot(state.requireHandle());
    } finally {
      state.lock.unlock();
    }
  }

  /** Structural quality recommendations across loaded specs. Advisory only.
   *
   * @return quality recommendations
   */
  public List<Recommendation> quality() {
    String json;
    state.lock.lock();
    try {
      json = Native.quality(state.requireHandle());
    } finally {
      state.lock.unlock();
    }
    return JsonSupport.parseRecommendations(json);
  }

  /** Releases native resources. */
  @Override
  public void close() {
    state.lock.lock();
    try {
      state.destroy();
    } finally {
      state.lock.unlock();
    }
    // Outside lock: destroy() already ran; clean() deregisters the cleanable to prevent
    // duplicate cleanup by the Cleaner thread.
    cleanable.clean();
  }

  /**
   * Native handle lifecycle shared by Engine methods and the Cleaner action. {@code handle == 0}
   * means destroyed. All reads/writes happen under {@link #lock}.
   */
  private static final class NativeState {
    /** Mutex for {@link #handle} reads and writes. */
    final ReentrantLock lock = new ReentrantLock();
    /** Native engine handle; zero after destroy. */
    private long handle;

    /**
     * Creates state wrapping a live native handle.
     *
     * @param handle native engine handle
     */
    NativeState(long handle) {
      this.handle = handle;
    }

    /** Returns the live handle or throws if closed.
     *
     * @return native engine handle
     */
    long requireHandle() {
      if (handle == 0L) {
        throw new LemmaBugError("BUG: Engine used after close");
      }
      return handle;
    }

    /** Destroys the native handle if not already destroyed. */
    void destroy() {
      if (handle == 0L) {
        return;
      }
      long toDestroy = handle;
      handle = 0L;
      Native.destroy(toDestroy);
    }
  }

  /** {@link Cleaner} action that destroys {@link NativeState} when the engine is unreachable. */
  private static final class DestroyAction implements Runnable {
    /** State to destroy on cleanup. */
    private final NativeState state;

    /**
     * Registers cleanup for {@code state}.
     *
     * @param state native state to destroy
     */
    DestroyAction(NativeState state) {
      this.state = state;
    }

    /** Destroys the native engine handle. */
    @Override
    public void run() {
      state.lock.lock();
      try {
        state.destroy();
      } finally {
        state.lock.unlock();
      }
    }
  }
}
