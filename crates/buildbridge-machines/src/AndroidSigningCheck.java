// Fixed JDK-only helper. Credentials arrive through length-framed stdin, never arguments.
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.security.*;
import java.security.cert.*;
import java.security.interfaces.*;
import java.util.*;
import java.util.jar.*;

class AndroidSigningCheck {
    static class CheckFailure extends Exception {
        final String code;
        CheckFailure(String code) { this.code = code; }
    }

    static byte[] field(DataInputStream input, int maximum) throws Exception {
        int size = input.readInt();
        if (size < 1 || size > maximum) throw new CheckFailure("INPUT");
        byte[] value = input.readNBytes(size);
        if (value.length != size) throw new CheckFailure("INPUT");
        return value;
    }

    static String digest(String algorithm, byte[] bytes) throws Exception {
        return HexFormat.of().formatHex(MessageDigest.getInstance(algorithm).digest(bytes));
    }

    static void checkKey() throws Exception {
        DataInputStream input = new DataInputStream(System.in);
        byte[] encoded = field(input, 1024 * 1024);
        byte[] storeBytes = field(input, 512);
        byte[] keyBytes = field(input, 512);
        String alias = new String(field(input, 64), StandardCharsets.UTF_8);
        if (input.read() != -1 || !alias.matches("[A-Za-z0-9._-]{1,64}")) {
            throw new CheckFailure("INPUT");
        }
        char[] storePassword = new String(storeBytes, StandardCharsets.UTF_8).toCharArray();
        char[] keyPassword = new String(keyBytes, StandardCharsets.UTF_8).toCharArray();
        Path file = Files.createTempFile("buildbridge-key-", ".keystore");
        try {
            Files.write(file, encoded);
            KeyStore store;
            try { store = KeyStore.getInstance(file.toFile(), storePassword); }
            catch (Exception failure) { throw new CheckFailure("KEYSTORE_PASSWORD"); }
            if (!store.containsAlias(alias)) throw new CheckFailure("ALIAS");
            if (!store.isKeyEntry(alias)) throw new CheckFailure("PRIVATE_KEY");
            Key key;
            try { key = store.getKey(alias, keyPassword); }
            catch (Exception failure) { throw new CheckFailure("KEY_PASSWORD"); }
            if (!(key instanceof PrivateKey)) throw new CheckFailure("PRIVATE_KEY");
            if (!(store.getCertificate(alias) instanceof X509Certificate)) {
                throw new CheckFailure("CERTIFICATE");
            }
            X509Certificate certificate = (X509Certificate) store.getCertificate(alias);
            try { certificate.checkValidity(); }
            catch (CertificateExpiredException failure) { throw new CheckFailure("EXPIRED"); }
            catch (CertificateNotYetValidException failure) { throw new CheckFailure("NOT_YET_VALID"); }
            boolean[] usage = certificate.getKeyUsage();
            if (usage != null && !usage[0]) throw new CheckFailure("KEY_USAGE");
            String algorithm = key.getAlgorithm();
            String signatureAlgorithm;
            int bits;
            if (algorithm.equals("RSA") && key instanceof RSAPrivateKey
                && certificate.getPublicKey() instanceof RSAPublicKey) {
                bits = ((RSAPrivateKey) key).getModulus().bitLength();
                if (bits < 2048) throw new CheckFailure("KEY_SIZE");
                signatureAlgorithm = "SHA256withRSA";
            } else if (algorithm.equals("EC") && key instanceof ECPrivateKey
                && certificate.getPublicKey() instanceof ECPublicKey) {
                bits = ((ECPrivateKey) key).getParams().getCurve().getField().getFieldSize();
                if (bits < 256) throw new CheckFailure("KEY_SIZE");
                signatureAlgorithm = "SHA256withECDSA";
            } else { throw new CheckFailure("ALGORITHM"); }
            byte[] challenge = new byte[32];
            new SecureRandom().nextBytes(challenge);
            Signature signer = Signature.getInstance(signatureAlgorithm);
            signer.initSign((PrivateKey) key);
            signer.update(challenge);
            byte[] signature = signer.sign();
            signer.initVerify(certificate.getPublicKey());
            signer.update(challenge);
            if (!signer.verify(signature)) throw new CheckFailure("PRIVATE_KEY_MISMATCH");
            System.out.println("{\"keyAlias\":\"" + alias + "\",\"certificateSha256\":\""
                + digest("SHA-256", certificate.getEncoded()) + "\",\"certificateSha1\":\""
                + digest("SHA-1", certificate.getEncoded()) + "\",\"algorithm\":\"" + algorithm
                + "\",\"keyBits\":" + bits + ",\"validFromEpochSeconds\":"
                + certificate.getNotBefore().getTime() / 1000 + ",\"validUntilEpochSeconds\":"
                + certificate.getNotAfter().getTime() / 1000 + "}");
        } finally {
            Arrays.fill(encoded, (byte) 0);
            Arrays.fill(storeBytes, (byte) 0);
            Arrays.fill(keyBytes, (byte) 0);
            Arrays.fill(storePassword, '\0');
            Arrays.fill(keyPassword, '\0');
            Files.deleteIfExists(file);
        }
    }

    // Reading every content entry triggers JAR cryptographic verification. A signed
    // manifest alone is insufficient: reject unsigned additions and other signers too.
    static void checkJar(String path, String expectedCertificate) throws Exception {
        if (!expectedCertificate.matches("[0-9a-f]{64}")) throw new CheckFailure("INPUT");
        boolean foundContent = false;
        try (JarFile jar = new JarFile(path, true)) {
            Set<String> names = new HashSet<>();
            Enumeration<JarEntry> entries = jar.entries();
            byte[] buffer = new byte[65536];
            while (entries.hasMoreElements()) {
                JarEntry entry = entries.nextElement();
                if (!names.add(entry.getName())) throw new CheckFailure("JAR_CONTENT");
                if (entry.isDirectory()) continue;
                try (InputStream content = jar.getInputStream(entry)) {
                    while (content.read(buffer) != -1) { }
                }
                String name = entry.getName().toUpperCase(Locale.ROOT);
                if (name.equals("META-INF/MANIFEST.MF")
                    || (name.startsWith("META-INF/") && !name.substring(9).contains("/")
                        && (name.endsWith(".SF") || name.endsWith(".RSA")
                            || name.endsWith(".EC") || name.endsWith(".DSA")))) continue;
                CodeSigner[] signers = entry.getCodeSigners();
                if (signers == null || signers.length != 1) throw new CheckFailure("JAR_SIGNER");
                X509Certificate certificate = (X509Certificate)
                    signers[0].getSignerCertPath().getCertificates().get(0);
                certificate.checkValidity();
                if (!digest("SHA-256", certificate.getEncoded()).equals(expectedCertificate)) {
                    throw new CheckFailure("JAR_SIGNER");
                }
                foundContent = true;
            }
        }
        if (!foundContent) throw new CheckFailure("JAR_CONTENT");
        System.out.println("buildbridge verified every app bundle entry and its signing certificate.");
    }

    public static void main(String[] args) {
        try {
            if (args.length == 0) checkKey();
            else if (args.length == 3 && args[0].equals("verify-jar")) checkJar(args[1], args[2]);
            else throw new CheckFailure("INPUT");
        } catch (CheckFailure failure) {
            System.err.println("BUILDBRIDGE_SIGNING_ERROR:" + failure.code);
            System.exit(1);
        } catch (Exception failure) {
            // Never echo exception messages: providers can include credential material.
            System.err.println("BUILDBRIDGE_SIGNING_ERROR:VERIFICATION");
            System.exit(1);
        }
    }
}
