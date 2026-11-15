package com.lemmabase.lemma;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpServer;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.zip.GZIPOutputStream;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class NativeDownloadTest {
  private static final String TRIPLE = "x86_64-unknown-linux-gnu";
  private static final String LIB_NAME = "liblemma_jni.so";
  private static final String VERSION = "0.11.4-test";

  @Test
  void releaseAssetNameMatchesGithubReleaseLayout() {
    assertEquals(
        "liblemma_jni-v0.11.4-x86_64-unknown-linux-gnu.so.tar.gz",
        Native.releaseAssetName("0.11.4", "x86_64-unknown-linux-gnu", "liblemma_jni.so"));
    assertEquals(
        "liblemma_jni-v0.11.4-aarch64-apple-darwin.dylib.tar.gz",
        Native.releaseAssetName("0.11.4", "aarch64-apple-darwin", "liblemma_jni.dylib"));
    assertEquals(
        "lemma_jni-v0.11.4-x86_64-pc-windows-msvc.dll.tar.gz",
        Native.releaseAssetName("0.11.4", "x86_64-pc-windows-msvc", "lemma_jni.dll"));
  }

  @Test
  void extractSingleFileTarGzRoundTrip() throws Exception {
    byte[] payload = "lemma-jni-fixture".getBytes(StandardCharsets.UTF_8);
    byte[] archive = tarGzOneFile("liblemma_jni-v0.11.4-x86_64-unknown-linux-gnu.so", payload);
    assertArrayEquals(payload, Native.extractSingleFileTarGz(archive));
  }

  @Test
  void downloadToCacheVerifiesShaAndReuses(@TempDir Path tempDir) throws Exception {
    byte[] payload = "native-bytes-for-cache".getBytes(StandardCharsets.UTF_8);
    String sha = sha256Hex(payload);
    String asset =
        Native.releaseAssetName(VERSION, TRIPLE, LIB_NAME);
    byte[] archive = tarGzOneFile(asset.replace(".tar.gz", ""), payload);

    HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    String path = "/lemma-v" + VERSION + "/" + asset;
    server.createContext(
        path,
        exchange -> {
          byte[] body = archive;
          exchange.sendResponseHeaders(200, body.length);
          try (OutputStream out = exchange.getResponseBody()) {
            out.write(body);
          }
        });
    server.start();
    try {
      String base = "http://127.0.0.1:" + server.getAddress().getPort();
      Path cacheRoot = tempDir.resolve("cache");
      Path first =
          Native.downloadToCache(TRIPLE, LIB_NAME, VERSION, sha, base, cacheRoot);
      assertTrue(Files.isRegularFile(first));
      assertArrayEquals(payload, Files.readAllBytes(first));
      assertTrue(first.toString().contains(sha));

      Path second =
          Native.downloadToCache(TRIPLE, LIB_NAME, VERSION, sha, base, cacheRoot);
      assertEquals(first, second);
    } finally {
      server.stop(0);
    }
  }

  @Test
  void downloadToCacheRejectsChecksumMismatch(@TempDir Path tempDir) throws Exception {
    byte[] payload = "wrong-bytes".getBytes(StandardCharsets.UTF_8);
    String asset = Native.releaseAssetName(VERSION, TRIPLE, LIB_NAME);
    byte[] archive = tarGzOneFile(asset.replace(".tar.gz", ""), payload);

    HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    String path = "/lemma-v" + VERSION + "/" + asset;
    server.createContext(
        path,
        exchange -> {
          exchange.sendResponseHeaders(200, archive.length);
          try (OutputStream out = exchange.getResponseBody()) {
            out.write(archive);
          }
        });
    server.start();
    try {
      String base = "http://127.0.0.1:" + server.getAddress().getPort();
      LemmaNativeException error =
          assertThrows(
              LemmaNativeException.class,
              () ->
                  Native.downloadToCache(
                      TRIPLE,
                      LIB_NAME,
                      VERSION,
                      "0".repeat(64),
                      base,
                      tempDir.resolve("cache")));
      assertTrue(error.getMessage().contains("SHA-256 mismatch"), error.getMessage());
    } finally {
      server.stop(0);
    }
  }

  private static byte[] tarGzOneFile(String entryName, byte[] payload) throws IOException {
    byte[] tar = tarOneFile(entryName, payload);
    ByteArrayOutputStream bos = new ByteArrayOutputStream();
    try (GZIPOutputStream gzip = new GZIPOutputStream(bos)) {
      gzip.write(tar);
    }
    return bos.toByteArray();
  }

  private static byte[] tarOneFile(String entryName, byte[] payload) {
    if (entryName.length() >= 100) {
      throw new IllegalArgumentException("tar name too long for ustar name field");
    }
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    byte[] header = new byte[512];
    byte[] nameBytes = entryName.getBytes(StandardCharsets.UTF_8);
    System.arraycopy(nameBytes, 0, header, 0, nameBytes.length);
    // mode
    writeOctal(header, 100, 8, 0644);
    // uid / gid
    writeOctal(header, 108, 8, 0);
    writeOctal(header, 116, 8, 0);
    // size
    writeOctal(header, 124, 12, payload.length);
    // mtime
    writeOctal(header, 136, 12, 0);
    // checksum field filled with spaces while computing
    for (int i = 148; i < 156; i++) {
      header[i] = ' ';
    }
    header[156] = '0';
    // magic / version
    byte[] magic = "ustar".getBytes(StandardCharsets.US_ASCII);
    System.arraycopy(magic, 0, header, 257, magic.length);
    header[262] = '0';
    header[263] = '0';
    int checksum = 0;
    for (byte b : header) {
      checksum += b & 0xff;
    }
    writeOctal(header, 148, 8, checksum);
    out.writeBytes(header);
    out.writeBytes(payload);
    int padding = (512 - (payload.length % 512)) % 512;
    out.writeBytes(new byte[padding]);
    // two zero blocks end the archive
    out.writeBytes(new byte[1024]);
    return out.toByteArray();
  }

  private static void writeOctal(byte[] header, int offset, int length, long value) {
    String octal = Long.toOctalString(value);
    // classic tar: digits then NUL, left-padded with '0'
    int digitSpace = length - 1;
    if (octal.length() > digitSpace) {
      throw new IllegalArgumentException("octal value too wide");
    }
    int pad = digitSpace - octal.length();
    for (int i = 0; i < pad; i++) {
      header[offset + i] = '0';
    }
    byte[] digits = octal.getBytes(StandardCharsets.US_ASCII);
    System.arraycopy(digits, 0, header, offset + pad, digits.length);
    header[offset + digitSpace] = 0;
  }

  private static String sha256Hex(byte[] bytes) throws NoSuchAlgorithmException {
    return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
  }
}
