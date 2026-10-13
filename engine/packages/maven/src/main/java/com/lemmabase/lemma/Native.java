package com.lemmabase.lemma;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.URISyntaxException;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Locale;

/** JNI declarations and native library load. */
final class Native {
  static {
    loadLibrary();
  }

  private Native() {}

  static native String version();

  static native long create();

  static native long createWithLimits(String limitsJson);

  static native long fromSnapshot(byte[] bytes);

  static native void destroy(long handle);

  static native void load(long handle, String code);

  static native void loadLabeled(long handle, String[] labels, String[] codes);

  /** Allocates an install handle and stores the first step. {@code limitsJson} may be null. */
  static native long installStart(String repository, String limitsJson);

  /** Current step JSON: {@code {"fetch":{...}}} or {@code {"finished":{...}}}. */
  static native String installStep(long handle);

  /**
   * Advance with an HTTP response. Returns the next step JSON. Caller must
   * {@link #installFree} the handle when done (including after a finished step).
   */
  static native String installRespond(
      long handle, int status, String headersJson, String body);

  /**
   * Advance with a transport failure. Returns the next step JSON. Caller must
   * {@link #installFree} the handle when done (including after a finished step).
   */
  static native String installFail(long handle, String message);

  /** Frees an install handle. Safe to call exactly once per {@link #installStart}. */
  static native void installFree(long handle);

  static native String specs(long handle);

  static native String show(long handle, String repository, String spec, String effective);

  static native String source(long handle, String repository, String spec, String effective);

  static native String run(
      long handle,
      String repository,
      String spec,
      String effective,
      java.util.Map<String, ?> data,
      String[] rules,
      boolean explain);

  static native void remove(long handle, String repository, String spec, String effective);

  static native void update(
      long handle, String repository, String code, String attribute);

  static native String format(String code);

  static native String limits(long handle);

  static native byte[] snapshot(long handle);

  static native String quality(long handle);

  private static void loadLibrary() {
    String triple = rustTargetTriple();
    String libName = libraryFileName();

    String propertyOverride = System.getProperty("lemma.native.library");
    if (propertyOverride != null && !propertyOverride.isBlank()) {
      Path overridePath = Path.of(propertyOverride);
      if (!Files.isRegularFile(overridePath)) {
        throw new LemmaNativeException(
            "lemma.native.library set to '" + propertyOverride + "' but it is not a regular file");
      }
      System.load(overridePath.toAbsolutePath().toString());
      return;
    }

    String envOverride = System.getenv("LEMMA_JNI_LIBRARY");
    if (envOverride != null && !envOverride.isBlank()) {
      Path overridePath = Path.of(envOverride);
      if (!Files.isRegularFile(overridePath)) {
        throw new LemmaNativeException(
            "LEMMA_JNI_LIBRARY set to '" + envOverride + "' but it is not a regular file");
      }
      System.load(overridePath.toAbsolutePath().toString());
      return;
    }

    String resourcePath = "native/" + triple + "/" + libName;
    URL resourceUrl = Native.class.getClassLoader().getResource(resourcePath);

    if (resourceUrl != null) {
      String protocol = resourceUrl.getProtocol();
      if ("file".equals(protocol)) {
        try {
          Path filePath = Path.of(resourceUrl.toURI());
          System.load(filePath.toAbsolutePath().toString());
          return;
        } catch (URISyntaxException e) {
          throw new LemmaNativeException("invalid resource URI: " + resourceUrl, e);
        }
      } else if ("jar".equals(protocol)) {
        loadFromJar(resourcePath, triple, libName);
        return;
      } else {
        throw new LemmaNativeException("unsupported resource URL protocol: " + protocol);
      }
    }

    Path dev = discoverDevLibrary();
    if (dev != null) {
      System.load(dev.toAbsolutePath().toString());
      return;
    }

    throw new LemmaNativeException(
        "native library not found. Checked: "
            + "lemma.native.library property, "
            + "LEMMA_JNI_LIBRARY env, "
            + "resource '"
            + resourcePath
            + "', "
            + "cargo target directories");
  }

  private static void loadFromJar(String resourcePath, String triple, String libName) {
    String version = getImplementationVersion();
    Path cacheRoot = getCacheRoot();
    Path cachedLib = extractToCache(resourcePath, triple, libName, version, cacheRoot);
    System.load(cachedLib.toAbsolutePath().toString());
  }

  static Path extractToCache(
      String resourcePath, String triple, String libName, String version, Path cacheRoot) {
    byte[] bytes = readResourceBytes(resourcePath);
    String contentHash = sha256Hex(bytes);
    Path cacheDir =
        cacheRoot.resolve("lemma-jni").resolve(version + "-" + triple).resolve(contentHash);
    Path cachedLib = cacheDir.resolve(libName);

    if (Files.isRegularFile(cachedLib)) {
      return cachedLib;
    }

    try {
      Files.createDirectories(cacheDir);
    } catch (IOException e) {
      throw new LemmaNativeException(
          "failed to create cache directory '"
              + cacheDir
              + "' (override with lemma.native.cache.dir property)",
          e);
    }

    Path tempFile;
    try {
      tempFile = Files.createTempFile(cacheDir, "lemma_jni_", ".tmp");
    } catch (IOException e) {
      throw new LemmaNativeException(
          "failed to create temp file in '"
              + cacheDir
              + "' (override with lemma.native.cache.dir property)",
          e);
    }

    try (OutputStream out = Files.newOutputStream(tempFile)) {
      out.write(bytes);
    } catch (IOException e) {
      try {
        Files.deleteIfExists(tempFile);
      } catch (IOException ignored) {
      }
      throw new LemmaNativeException("failed to extract native library", e);
    }

    try {
      Files.move(tempFile, cachedLib, StandardCopyOption.ATOMIC_MOVE);
    } catch (FileAlreadyExistsException e) {
      try {
        Files.deleteIfExists(tempFile);
      } catch (IOException ignored) {
      }
    } catch (IOException e) {
      try {
        Files.deleteIfExists(tempFile);
      } catch (IOException ignored) {
      }
      throw new LemmaNativeException("failed to move extracted library to cache", e);
    }

    return cachedLib;
  }

  private static byte[] readResourceBytes(String resourcePath) {
    try (InputStream in = Native.class.getClassLoader().getResourceAsStream(resourcePath)) {
      if (in == null) {
        throw new LemmaNativeException("resource disappeared: " + resourcePath);
      }
      return in.readAllBytes();
    } catch (IOException e) {
      throw new LemmaNativeException("failed to read native library resource: " + resourcePath, e);
    }
  }

  private static String sha256Hex(byte[] bytes) {
    try {
      MessageDigest digest = MessageDigest.getInstance("SHA-256");
      return HexFormat.of().formatHex(digest.digest(bytes));
    } catch (NoSuchAlgorithmException e) {
      throw new LemmaBugError("BUG: SHA-256 MessageDigest unavailable: " + e);
    }
  }

  static String implementationVersion() {
    return getImplementationVersion();
  }

  private static String getImplementationVersion() {
    try (InputStream in = Native.class.getResourceAsStream("engine.version")) {
      if (in == null) {
        throw new LemmaNativeException(
            "engine.version resource missing; cannot determine version for cache key");
      }
      String version = new String(in.readAllBytes(), StandardCharsets.UTF_8).trim();
      if (version.isBlank()) {
        throw new LemmaNativeException(
            "engine.version blank; cannot determine version for cache key");
      }
      return version;
    } catch (IOException e) {
      throw new LemmaNativeException("failed to read engine.version", e);
    }
  }

  private static Path getCacheRoot() {
    String cacheDirProperty = System.getProperty("lemma.native.cache.dir");
    if (cacheDirProperty != null && !cacheDirProperty.isBlank()) {
      return Path.of(cacheDirProperty);
    }
    String userHome = System.getProperty("user.home");
    if (userHome == null || userHome.isBlank()) {
      throw new LemmaNativeException(
          "user.home property is not set; override with lemma.native.cache.dir property");
    }
    return Path.of(userHome, ".cache");
  }

  private static Path discoverDevLibrary() {
    String name = libraryFileName();
    String cargoTarget = System.getenv("CARGO_TARGET_DIR");
    if (cargoTarget != null && !cargoTarget.isBlank()) {
      Path[] fromEnv =
          new Path[] {
            Path.of(cargoTarget, "debug", name),
            Path.of(cargoTarget, "release", name),
          };
      for (Path candidate : fromEnv) {
        if (Files.isRegularFile(candidate)) {
          return candidate;
        }
      }
    }
    Path[] candidates =
        new Path[] {
          Path.of("target/debug").resolve(name),
          Path.of("target/release").resolve(name),
          Path.of("../../../../target/debug").resolve(name),
          Path.of("../../../../target/release").resolve(name),
          Path.of(System.getProperty("user.dir"), "target/debug", name),
          Path.of(System.getProperty("user.dir"), "target/release", name),
        };
    for (Path candidate : candidates) {
      if (Files.isRegularFile(candidate)) {
        return candidate;
      }
    }
    return null;
  }

  private static String libraryFileName() {
    String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
    if (os.contains("mac") || os.contains("darwin")) {
      return "liblemma_jni.dylib";
    }
    if (os.contains("win")) {
      return "lemma_jni.dll";
    }
    return "liblemma_jni.so";
  }

  private static String rustTargetTriple() {
    String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
    String arch = System.getProperty("os.arch", "").toLowerCase(Locale.ROOT);
    String rustArch =
        switch (arch) {
          case "amd64", "x86_64" -> "x86_64";
          case "aarch64", "arm64" -> "aarch64";
          default ->
              throw new LemmaNativeException(
                  "unsupported CPU architecture for lemma_jni: " + arch);
        };
    if (os.contains("mac") || os.contains("darwin")) {
      return rustArch + "-apple-darwin";
    }
    if (os.contains("win")) {
      return rustArch + "-pc-windows-msvc";
    }
    if (os.contains("linux")) {
      return rustArch + "-unknown-linux-gnu";
    }
    throw new LemmaNativeException("unsupported OS for lemma_jni: " + os);
  }
}
