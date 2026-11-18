package com.lemmabase.lemma;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.URL;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Duration;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import java.util.Properties;
import java.util.zip.GZIPInputStream;

/** JNI declarations and native library load. */
final class Native {
  private static final String CHECKSUMS_RESOURCE = "native-checksums.properties";
  private static final String DEFAULT_DOWNLOAD_BASE =
      "https://github.com/lemma/lemma/releases/download";

  static {
    loadLibrary();
  }

  private Native() {}

  static native String version();

  static native long create();

  static native long createWithLimits(String limitsJson);


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


  static native String quality(long handle);

  /** JNI load. {@code System.load} is restricted; the process must enable native access. */
  @SuppressWarnings("restricted")
  private static void loadNative(Path library) {
    System.load(library.toAbsolutePath().toString());
  }

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
      loadNative(overridePath);
      return;
    }

    String envOverride = System.getenv("LEMMA_JNI_LIBRARY");
    if (envOverride != null && !envOverride.isBlank()) {
      Path overridePath = Path.of(envOverride);
      if (!Files.isRegularFile(overridePath)) {
        throw new LemmaNativeException(
            "LEMMA_JNI_LIBRARY set to '" + envOverride + "' but it is not a regular file");
      }
      loadNative(overridePath);
      return;
    }

    String resourcePath = "native/" + triple + "/" + libName;
    URL resourceUrl = Native.class.getClassLoader().getResource(resourcePath);

    if (resourceUrl != null) {
      String protocol = resourceUrl.getProtocol();
      if ("file".equals(protocol)) {
        try {
          Path filePath = Path.of(resourceUrl.toURI());
          loadNative(filePath);
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

    String version = getImplementationVersion();
    Map<String, String> checksums = readChecksums();
    String expectedSha = checksums.get(triple);
    if (expectedSha != null && downloadsEnabled()) {
      Path cacheRoot = getCacheRoot();
      Path cached =
          downloadToCache(triple, libName, version, expectedSha, downloadBaseUrl(), cacheRoot);
      loadNative(cached);
      return;
    }

    Path dev = discoverDevLibrary();
    if (dev != null) {
      loadNative(dev);
      return;
    }

    String checksumHint =
        checksums.isEmpty()
            ? "no " + CHECKSUMS_RESOURCE + " in the jar"
            : "checksum entry missing for triple '" + triple + "'";
    throw new LemmaNativeException(
        "native library not found. Checked: "
            + "lemma.native.library property, "
            + "LEMMA_JNI_LIBRARY env, "
            + "resource '"
            + resourcePath
            + "', "
            + "GitHub release download ("
            + checksumHint
            + "), "
            + "cargo target directories. "
            + "Offline: set lemma.native.library or LEMMA_JNI_LIBRARY to a built lemma_jni.");
  }

  private static void loadFromJar(String resourcePath, String triple, String libName) {
    String version = getImplementationVersion();
    Path cacheRoot = getCacheRoot();
    Path cachedLib = extractToCache(resourcePath, triple, libName, version, cacheRoot);
    loadNative(cachedLib);
  }

  static Path extractToCache(
      String resourcePath, String triple, String libName, String version, Path cacheRoot) {
    byte[] bytes = readResourceBytes(resourcePath);
    return writeToCache(bytes, triple, libName, version, cacheRoot);
  }

  /**
   * Download the release {@code .tar.gz} for {@code triple}, verify SHA-256 of the extracted
   * library, and cache it. Package-visible for tests.
   */
  static Path downloadToCache(
      String triple,
      String libName,
      String version,
      String expectedSha256,
      String downloadBase,
      Path cacheRoot) {
    String normalizedExpected = expectedSha256.toLowerCase(Locale.ROOT);
    Path cacheDir =
        cacheRoot.resolve("lemma-jni").resolve(version + "-" + triple).resolve(normalizedExpected);
    Path cachedLib = cacheDir.resolve(libName);
    if (Files.isRegularFile(cachedLib)) {
      return cachedLib;
    }

    String assetName = releaseAssetName(version, triple, libName);
    URI uri = releaseDownloadUri(downloadBase, version, assetName);
    byte[] archiveBytes = httpGetBytes(uri);
    byte[] libraryBytes = extractSingleFileTarGz(archiveBytes);
    String actualSha = sha256Hex(libraryBytes);
    if (!actualSha.equals(normalizedExpected)) {
      throw new LemmaNativeException(
          "downloaded native SHA-256 mismatch for "
              + triple
              + ": expected "
              + normalizedExpected
              + ", got "
              + actualSha
              + " from "
              + uri);
    }
    return writeToCache(libraryBytes, triple, libName, version, cacheRoot);
  }

  static Path writeToCache(
      byte[] bytes, String triple, String libName, String version, Path cacheRoot) {
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

  static Map<String, String> readChecksums() {
    try (InputStream in = Native.class.getResourceAsStream(CHECKSUMS_RESOURCE)) {
      if (in == null) {
        return Map.of();
      }
      Properties props = new Properties();
      props.load(in);
      Map<String, String> out = new LinkedHashMap<>();
      for (String name : props.stringPropertyNames()) {
        String value = props.getProperty(name);
        if (value != null && !value.isBlank()) {
          out.put(name.trim(), value.trim().toLowerCase(Locale.ROOT));
        }
      }
      return Map.copyOf(out);
    } catch (IOException e) {
      throw new LemmaNativeException("failed to read " + CHECKSUMS_RESOURCE, e);
    }
  }

  static String releaseAssetName(String version, String triple, String libName) {
    if (libName.endsWith(".dll")) {
      return "lemma_jni-v" + version + "-" + triple + ".dll.tar.gz";
    }
    if (libName.endsWith(".dylib")) {
      return "liblemma_jni-v" + version + "-" + triple + ".dylib.tar.gz";
    }
    if (libName.endsWith(".so")) {
      return "liblemma_jni-v" + version + "-" + triple + ".so.tar.gz";
    }
    throw new LemmaBugError("BUG: unknown lemma_jni library file name: " + libName);
  }

  static URI releaseDownloadUri(String downloadBase, String version, String assetName) {
    String base = downloadBase.endsWith("/") ? downloadBase.substring(0, downloadBase.length() - 1) : downloadBase;
    try {
      return URI.create(base + "/lemma-v" + version + "/" + assetName);
    } catch (IllegalArgumentException e) {
      throw new LemmaNativeException(
          "invalid native download URI from base '" + downloadBase + "': " + e.getMessage(), e);
    }
  }

  private static String downloadBaseUrl() {
    String property = System.getProperty("lemma.native.download.base");
    if (property != null && !property.isBlank()) {
      return property.trim();
    }
    return DEFAULT_DOWNLOAD_BASE;
  }

  private static boolean downloadsEnabled() {
    String property = System.getProperty("lemma.native.download");
    if (property == null || property.isBlank()) {
      return true;
    }
    return !property.equalsIgnoreCase("false") && !property.equals("0");
  }

  private static byte[] httpGetBytes(URI uri) {
    HttpClient client =
        HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(30))
            .followRedirects(HttpClient.Redirect.NORMAL)
            .build();
    HttpRequest request =
        HttpRequest.newBuilder(uri).timeout(Duration.ofMinutes(2)).GET().build();
    try {
      HttpResponse<byte[]> response = client.send(request, HttpResponse.BodyHandlers.ofByteArray());
      int status = response.statusCode();
      if (status != 200) {
        throw new LemmaNativeException(
            "failed to download native library from " + uri + ": HTTP " + status);
      }
      return response.body();
    } catch (IOException e) {
      throw new LemmaNativeException("failed to download native library from " + uri, e);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new LemmaNativeException("interrupted downloading native library from " + uri, e);
    }
  }

  /**
   * Extract the sole regular file from a gzip-compressed ustar archive. Package-visible for tests.
   */
  static byte[] extractSingleFileTarGz(byte[] archiveBytes) {
    try (InputStream raw = new java.io.ByteArrayInputStream(archiveBytes);
        GZIPInputStream gzip = new GZIPInputStream(raw)) {
      return extractSingleFileTar(gzip);
    } catch (IOException e) {
      throw new LemmaNativeException("failed to gunzip native release archive", e);
    }
  }

  static byte[] extractSingleFileTar(InputStream in) throws IOException {
    byte[] header = new byte[512];
    byte[] found = null;
    String foundName = null;
    while (true) {
      int read = readFully(in, header);
      if (read == 0) {
        break;
      }
      if (read != 512) {
        throw new LemmaNativeException("truncated tar header while reading native archive");
      }
      if (isZeroBlock(header)) {
        break;
      }
      String name = tarName(header);
      long size = tarSize(header);
      char type = (char) (header[156] & 0xff);
      if (type == '\0' || type == '0') {
        byte[] payload = in.readNBytes((int) size);
        if (payload.length != size) {
          throw new LemmaNativeException(
              "truncated tar payload for '" + name + "' in native archive");
        }
        skipTarPadding(in, size);
        if (found != null) {
          throw new LemmaNativeException(
              "native archive contains multiple files ('"
                  + foundName
                  + "' and '"
                  + name
                  + "'); expected exactly one");
        }
        found = payload;
        foundName = name;
      } else if (type == '5') {
        // directory: no payload
        skipTarPadding(in, size);
      } else {
        throw new LemmaNativeException(
            "unsupported tar entry type '" + type + "' for '" + name + "' in native archive");
      }
    }
    if (found == null) {
      throw new LemmaNativeException("native archive contained no regular file");
    }
    return found;
  }

  private static int readFully(InputStream in, byte[] buffer) throws IOException {
    int offset = 0;
    while (offset < buffer.length) {
      int n = in.read(buffer, offset, buffer.length - offset);
      if (n < 0) {
        return offset;
      }
      offset += n;
    }
    return offset;
  }

  private static boolean isZeroBlock(byte[] header) {
    for (byte b : header) {
      if (b != 0) {
        return false;
      }
    }
    return true;
  }

  private static String tarName(byte[] header) {
    int end = 0;
    while (end < 100 && header[end] != 0) {
      end++;
    }
    return new String(header, 0, end, StandardCharsets.UTF_8);
  }

  private static long tarSize(byte[] header) {
    String octal = new String(header, 124, 12, StandardCharsets.UTF_8).trim();
    if (octal.isEmpty()) {
      return 0;
    }
    // Some writers null-terminate inside the field.
    int end = 0;
    while (end < octal.length() && octal.charAt(end) != 0) {
      end++;
    }
    String cleaned = octal.substring(0, end).trim();
    if (cleaned.isEmpty()) {
      return 0;
    }
    try {
      return Long.parseLong(cleaned, 8);
    } catch (NumberFormatException e) {
      throw new LemmaNativeException("invalid tar size field: '" + cleaned + "'", e);
    }
  }

  private static void skipTarPadding(InputStream in, long size) throws IOException {
    long padding = (512 - (size % 512)) % 512;
    if (padding == 0) {
      return;
    }
    long skipped = in.skip(padding);
    if (skipped != padding) {
      // skip may be short; fall back to read
      long remaining = padding - skipped;
      while (remaining > 0) {
        long n = in.skip(remaining);
        if (n == 0) {
          if (in.read() < 0) {
            throw new LemmaNativeException("truncated tar padding in native archive");
          }
          remaining--;
        } else {
          remaining -= n;
        }
      }
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
