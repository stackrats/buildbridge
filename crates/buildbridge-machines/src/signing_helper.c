#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>

#include <arpa/inet.h>
#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define MAX_SECRET_BYTES 512
#define MAX_CERTIFICATE_BYTES (32 * 1024 * 1024)

/*
 * Apple uses this SPI in its own partition-list command.
 * Declaring it here lets the helper supply the dedicated keychain password
 * from memory instead of exposing it through `security -k` process arguments.
 */
extern OSStatus SecKeychainItemSetAccessWithPassword(
    SecKeychainItemRef item,
    SecAccessRef access,
    UInt32 password_length,
    const void *password
);

/*
 * xcodebuild opens a workspace with -workspace and a bare project with -project; the
 * container path names which one it is by its extension.
 */
static const char *xcode_container_flag(const char *path) {
    size_t length = strlen(path);
    const char *suffix = ".xcodeproj";
    size_t suffix_length = strlen(suffix);
    while (length > 0 && path[length - 1] == '/') {
        length--;
    }
    if (length >= suffix_length && strncmp(path + length - suffix_length, suffix, suffix_length) == 0) {
        return "-project";
    }
    return "-workspace";
}

static int read_exact(void *buffer, size_t length) {
    unsigned char *cursor = buffer;

    while (length > 0) {
        size_t count = fread(cursor, 1, length, stdin);
        if (count == 0) {
            return 0;
        }
        cursor += count;
        length -= count;
    }

    return 1;
}

static unsigned char *read_secret(uint32_t *length) {
    uint32_t network_length = 0;
    if (!read_exact(&network_length, sizeof(network_length))) {
        return NULL;
    }

    *length = ntohl(network_length);
    if (*length == 0 || *length > MAX_SECRET_BYTES) {
        return NULL;
    }

    unsigned char *secret = calloc(*length, 1);
    if (secret == NULL || !read_exact(secret, *length)) {
        free(secret);
        return NULL;
    }

    return secret;
}

static unsigned char *read_file(const char *path, size_t *length) {
    FILE *file = fopen(path, "rb");
    if (file == NULL || fseek(file, 0, SEEK_END) != 0) {
        if (file != NULL) fclose(file);
        return NULL;
    }

    long size = ftell(file);
    if (size <= 0 || size > MAX_CERTIFICATE_BYTES || fseek(file, 0, SEEK_SET) != 0) {
        fclose(file);
        return NULL;
    }

    unsigned char *bytes = malloc((size_t)size);
    if (bytes == NULL || fread(bytes, 1, (size_t)size, file) != (size_t)size) {
        free(bytes);
        fclose(file);
        return NULL;
    }

    fclose(file);
    *length = (size_t)size;
    return bytes;
}

static int write_certificate(const char *path, SecCertificateRef certificate) {
    CFDataRef data = SecCertificateCopyData(certificate);
    if (data == NULL) return 0;

    FILE *file = fopen(path, "wb");
    int success = file != NULL &&
        fwrite(CFDataGetBytePtr(data), 1, (size_t)CFDataGetLength(data), file) ==
            (size_t)CFDataGetLength(data);
    if (file != NULL) fclose(file);
    CFRelease(data);
    return success;
}

static int security_failure(const char *step, OSStatus status) {
    fprintf(stderr, "%s:%d\n", step, (int)status);
    return 1;
}

static void secure_zero(void *buffer, size_t length) {
    volatile unsigned char *cursor = buffer;
    while (length-- > 0) {
        *cursor++ = 0;
    }
}

static CFStringRef copy_hex_string(CFDataRef data) {
    CFIndex byte_count = CFDataGetLength(data);
    CFMutableStringRef hex = CFStringCreateMutable(NULL, byte_count * 2);
    if (hex == NULL) return NULL;

    static const char digits[] = "0123456789abcdef";
    const UInt8 *bytes = CFDataGetBytePtr(data);
    for (CFIndex index = 0; index < byte_count; index++) {
        char pair[] = {
            digits[bytes[index] >> 4],
            digits[bytes[index] & 0x0f],
            '\0',
        };
        CFStringAppendCString(hex, pair, kCFStringEncodingASCII);
    }

    return hex;
}

static OSStatus authorize_identity_for_apple_signing(
    SecIdentityRef identity,
    const unsigned char *keychain_password,
    uint32_t keychain_password_length
) {
    OSStatus status = errSecSuccess;
    SecKeyRef private_key = NULL;
    SecAccessRef key_access = NULL;
    CFArrayRef acl_list = NULL;
    CFArrayRef partition_ids = NULL;
    CFMutableDictionaryRef partition_dictionary = NULL;
    CFDataRef partition_xml = NULL;
    CFStringRef partition_description = NULL;
    bool found_partition_acl = false;

    status = SecIdentityCopyPrivateKey(identity, &private_key);
    if (status != errSecSuccess || private_key == NULL) goto cleanup;

    status = SecKeychainItemCopyAccess((SecKeychainItemRef)private_key, &key_access);
    if (status != errSecSuccess || key_access == NULL) goto cleanup;

    status = SecAccessCopyACLList(key_access, &acl_list);
    if (status != errSecSuccess || acl_list == NULL) goto cleanup;

    const void *partition_values[] = {CFSTR("apple-tool:"), CFSTR("apple:")};
    partition_ids = CFArrayCreate(
        NULL, partition_values, 2, &kCFTypeArrayCallBacks
    );
    partition_dictionary = CFDictionaryCreateMutable(
        NULL,
        1,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks
    );
    if (partition_ids == NULL || partition_dictionary == NULL) {
        status = errSecAllocate;
        goto cleanup;
    }
    CFDictionarySetValue(
        partition_dictionary, CFSTR("Partitions"), partition_ids
    );
    partition_xml = CFPropertyListCreateData(
        NULL,
        partition_dictionary,
        kCFPropertyListXMLFormat_v1_0,
        0,
        NULL
    );
    if (partition_xml == NULL) {
        status = errSecAllocate;
        goto cleanup;
    }
    partition_description = copy_hex_string(partition_xml);
    if (partition_description == NULL) {
        status = errSecAllocate;
        goto cleanup;
    }

    for (CFIndex index = 0; index < CFArrayGetCount(acl_list); index++) {
        SecACLRef acl = (SecACLRef)CFArrayGetValueAtIndex(acl_list, index);
        CSSM_ACL_AUTHORIZATION_TAG tags[64];
        uint32 tag_count = (uint32)(sizeof(tags) / sizeof(*tags));
        status = SecACLGetAuthorizations(acl, tags, &tag_count);
        if (status != errSecSuccess) goto cleanup;

        for (uint32 tag_index = 0; tag_index < tag_count; tag_index++) {
            if (tags[tag_index] != CSSM_ACL_AUTHORIZATION_PARTITION_ID) continue;

            CFArrayRef application_list = NULL;
            CFStringRef prompt_description = NULL;
            CSSM_ACL_KEYCHAIN_PROMPT_SELECTOR prompt_selector = {0};
            status = SecACLCopySimpleContents(
                acl,
                &application_list,
                &prompt_description,
                &prompt_selector
            );
            if (status == errSecSuccess) {
                status = SecACLSetSimpleContents(
                    acl,
                    application_list,
                    partition_description,
                    &prompt_selector
                );
            }
            if (prompt_description != NULL) CFRelease(prompt_description);
            if (application_list != NULL) CFRelease(application_list);
            if (status != errSecSuccess) goto cleanup;
            found_partition_acl = true;
        }
    }

    if (!found_partition_acl) {
        status = errSecItemNotFound;
        goto cleanup;
    }

    status = SecKeychainItemSetAccessWithPassword(
        (SecKeychainItemRef)private_key,
        key_access,
        keychain_password_length,
        keychain_password
    );

cleanup:
    if (partition_description != NULL) CFRelease(partition_description);
    if (partition_xml != NULL) CFRelease(partition_xml);
    if (partition_dictionary != NULL) CFRelease(partition_dictionary);
    if (partition_ids != NULL) CFRelease(partition_ids);
    if (acl_list != NULL) CFRelease(acl_list);
    if (key_access != NULL) CFRelease(key_access);
    if (private_key != NULL) CFRelease(private_key);
    return status;
}

static int run_tool(const char *path, char *const arguments[]) {
    pid_t child = fork();
    if (child < 0) return 0;
    if (child == 0) {
        execv(path, arguments);
        _exit(127);
    }

    int wait_status = 0;
    while (waitpid(child, &wait_status, 0) < 0) {
        if (errno != EINTR) return 0;
    }
    return WIFEXITED(wait_status) && WEXITSTATUS(wait_status) == 0;
}

static char *make_build_setting(const char *name, const char *value) {
    size_t length = strlen(name) + strlen(value) + 2;
    char *setting = calloc(length, 1);
    if (setting == NULL) return NULL;
    if (snprintf(setting, length, "%s=%s", name, value) < 0) {
        free(setting);
        return NULL;
    }
    return setting;
}

/*
 * Xcode's archive is told which keychain to sign from, because the settings file carries
 * `--keychain`. Its export is not: `xcodebuild -exportArchive` re-signs the app itself and
 * looks the identity up through the user's default keychain and search list. On a machine
 * nobody has logged into, the login keychain is both the default and locked, and that lookup
 * fails with errSecInternalComponent even though the identity is sitting unlocked in
 * buildbridge's own keychain.
 *
 * So for the length of the operation the signing keychain becomes the only keychain the user
 * has, and the default; both are put back afterwards, whichever way the operation ends. This
 * is the same thing Apple's own guidance has continuous integration do, and it is what makes
 * an export work without anyone signing in to the machine.
 */
static int take_over_keychain(
    SecKeychainRef keychain,
    SecKeychainRef *previous_default,
    CFArrayRef *previous_search_list
) {
    OSStatus status = SecKeychainCopyDefault(previous_default);
    if (status != errSecSuccess) *previous_default = NULL;
    status = SecKeychainCopySearchList(previous_search_list);
    if (status != errSecSuccess) *previous_search_list = NULL;

    CFMutableArrayRef only = CFArrayCreateMutable(NULL, 1, &kCFTypeArrayCallBacks);
    if (only == NULL) return 0;
    CFArrayAppendValue(only, keychain);
    status = SecKeychainSetSearchList(only);
    CFRelease(only);
    if (status != errSecSuccess) return 0;

    return SecKeychainSetDefault(keychain) == errSecSuccess;
}

static void restore_keychain(
    SecKeychainRef previous_default,
    CFArrayRef previous_search_list
) {
    if (previous_search_list != NULL) {
        SecKeychainSetSearchList(previous_search_list);
    }
    if (previous_default != NULL) {
        SecKeychainSetDefault(previous_default);
    }
}

static int run_signed_archive(int argc, char **argv) {
    if (argc != 11) {
        fprintf(stderr, "invalid_archive_arguments\n");
        return 1;
    }

    uint32_t keychain_password_length = 0;
    unsigned char *keychain_password = read_secret(&keychain_password_length);
    if (keychain_password == NULL) {
        fprintf(stderr, "invalid_secret_payload\n");
        return 1;
    }

    int result = 1;
    SecKeychainRef keychain = NULL;
    SecKeychainRef previous_default = NULL;
    CFArrayRef previous_search_list = NULL;
    int took_over_keychain = 0;
    char *keychain_flags = NULL;
    OSStatus status = SecKeychainOpen(argv[2], &keychain);
    if (status != errSecSuccess || keychain == NULL) {
        result = security_failure("open_keychain", status);
        goto cleanup;
    }
    status = SecKeychainUnlock(
        keychain,
        keychain_password_length,
        keychain_password,
        true
    );
    if (status != errSecSuccess) {
        result = security_failure("unlock_keychain", status);
        goto cleanup;
    }

    took_over_keychain = take_over_keychain(
        keychain, &previous_default, &previous_search_list
    );
    if (!took_over_keychain) {
        fprintf(stderr, "claim_keychain\n");
        goto cleanup;
    }

    size_t keychain_flags_length = strlen(argv[2]) + 41;
    char *keychain_flags_value = calloc(keychain_flags_length, 1);
    if (keychain_flags_value != NULL) {
        snprintf(
            keychain_flags_value,
            keychain_flags_length,
            "--keychain %s",
            argv[2]
        );
        keychain_flags = make_build_setting(
            "OTHER_CODE_SIGN_FLAGS",
            keychain_flags_value
        );
        secure_zero(keychain_flags_value, keychain_flags_length);
        free(keychain_flags_value);
    }
    if (keychain_flags == NULL) {
        fprintf(stderr, "prepare_archive_settings\n");
        goto cleanup;
    }

    printf("__BUILDBRIDGE_ARCHIVE__:archiving\n");
    fflush(stdout);
    char *archive_arguments[] = {
        argv[3],
        (char *)xcode_container_flag(argv[4]),
        argv[4],
        "-scheme",
        argv[5],
        "-configuration",
        "Release",
        "-destination",
        "generic/platform=iOS",
        "-archivePath",
        argv[6],
        "-derivedDataPath",
        argv[7],
        "-xcconfig",
        argv[8],
        "archive",
        keychain_flags,
        "COMPILER_INDEX_STORE_ENABLE=NO",
        NULL,
    };
    if (!run_tool(argv[3], archive_arguments)) {
        fprintf(stderr, "xcode_archive_failed\n");
        goto cleanup;
    }

    printf("__BUILDBRIDGE_ARCHIVE__:exporting\n");
    fflush(stdout);
    char *export_arguments[] = {
        argv[3],
        "-exportArchive",
        "-archivePath",
        argv[6],
        "-exportPath",
        argv[10],
        "-exportOptionsPlist",
        argv[9],
        NULL,
    };
    if (!run_tool(argv[3], export_arguments)) {
        fprintf(stderr, "xcode_export_failed\n");
        goto cleanup;
    }

    printf("__BUILDBRIDGE_ARCHIVE__:complete\n");
    fflush(stdout);
    result = 0;

cleanup:
    if (took_over_keychain) {
        restore_keychain(previous_default, previous_search_list);
    }
    if (previous_default != NULL) CFRelease(previous_default);
    if (previous_search_list != NULL) CFRelease(previous_search_list);
    if (keychain != NULL) {
        SecKeychainLock(keychain);
        CFRelease(keychain);
    }
    if (keychain_flags != NULL) free(keychain_flags);
    secure_zero(keychain_password, keychain_password_length);
    free(keychain_password);
    return result;
}

static int run_code_signing_probe(int argc, char **argv) {
    if (argc != 5) {
        fprintf(stderr, "invalid_probe_arguments\n");
        return 1;
    }

    uint32_t keychain_password_length = 0;
    unsigned char *keychain_password = read_secret(&keychain_password_length);
    if (keychain_password == NULL) {
        fprintf(stderr, "invalid_secret_payload\n");
        return 1;
    }

    int result = 1;
    SecKeychainRef keychain = NULL;
    OSStatus status = SecKeychainOpen(argv[2], &keychain);
    if (status != errSecSuccess || keychain == NULL) {
        result = security_failure("open_keychain", status);
        goto cleanup;
    }
    status = SecKeychainUnlock(
        keychain,
        keychain_password_length,
        keychain_password,
        true
    );
    if (status != errSecSuccess) {
        result = security_failure("unlock_keychain", status);
        goto cleanup;
    }

    char *sign_arguments[] = {
        "/usr/bin/codesign",
        "--force",
        "--sign",
        argv[3],
        "--keychain",
        argv[2],
        "--timestamp=none",
        argv[4],
        NULL,
    };
    if (!run_tool("/usr/bin/codesign", sign_arguments)) {
        fprintf(stderr, "codesign_probe_failed\n");
        goto cleanup;
    }

    char *verify_arguments[] = {
        "/usr/bin/codesign",
        "--verify",
        "--strict",
        argv[4],
        NULL,
    };
    if (!run_tool("/usr/bin/codesign", verify_arguments)) {
        fprintf(stderr, "codesign_verification_failed\n");
        goto cleanup;
    }

    printf("ok\n");
    result = 0;

cleanup:
    if (keychain != NULL) {
        SecKeychainLock(keychain);
        CFRelease(keychain);
    }
    secure_zero(keychain_password, keychain_password_length);
    free(keychain_password);
    return result;
}

static int open_and_unlock_keychain(
    const char *path,
    SecKeychainRef *keychain,
    unsigned char *password,
    uint32_t password_length
) {
    OSStatus status = SecKeychainOpen(path, keychain);
    if (status != errSecSuccess || *keychain == NULL) {
        return security_failure("open_keychain", status);
    }
    status = SecKeychainUnlock(*keychain, password_length, password, true);
    if (status != errSecSuccess) {
        return security_failure("unlock_keychain", status);
    }
    return 0;
}

static char *keychain_sign_flags(const char *keychain_path) {
    size_t length = strlen(keychain_path) + 41;
    char *value = calloc(length, 1);
    if (value == NULL) return NULL;
    snprintf(value, length, "--keychain %s", keychain_path);
    char *setting = make_build_setting("OTHER_CODE_SIGN_FLAGS", value);
    secure_zero(value, length);
    free(value);
    return setting;
}

/*
 * A Debug build for a physical device, signed with the development identity in the same
 * keychain the archive uses. No export: the product stays in DerivedData for devicectl.
 */
static int run_device_build(int argc, char **argv) {
    if (argc != 8) {
        fprintf(stderr, "invalid_device_build_arguments\n");
        return 1;
    }

    uint32_t keychain_password_length = 0;
    unsigned char *keychain_password = read_secret(&keychain_password_length);
    if (keychain_password == NULL) {
        fprintf(stderr, "invalid_secret_payload\n");
        return 1;
    }

    int result = 1;
    SecKeychainRef keychain = NULL;
    char *keychain_flags = NULL;
    int unlock = open_and_unlock_keychain(
        argv[2], &keychain, keychain_password, keychain_password_length
    );
    if (unlock != 0) {
        result = unlock;
        goto cleanup;
    }
    keychain_flags = keychain_sign_flags(argv[2]);
    if (keychain_flags == NULL) {
        fprintf(stderr, "prepare_build_settings\n");
        goto cleanup;
    }

    printf("__BUILDBRIDGE_DEVICE_BUILD__:building\n");
    fflush(stdout);
    char *build_arguments[] = {
        argv[3],
        (char *)xcode_container_flag(argv[4]),
        argv[4],
        "-scheme",
        argv[5],
        "-configuration",
        "Debug",
        "-destination",
        "generic/platform=iOS",
        "-derivedDataPath",
        argv[6],
        "-xcconfig",
        argv[7],
        "build",
        keychain_flags,
        "COMPILER_INDEX_STORE_ENABLE=NO",
        NULL,
    };
    if (!run_tool(argv[3], build_arguments)) {
        fprintf(stderr, "xcode_device_build_failed\n");
        goto cleanup;
    }

    printf("__BUILDBRIDGE_DEVICE_BUILD__:complete\n");
    fflush(stdout);
    result = 0;

cleanup:
    if (keychain != NULL) {
        SecKeychainLock(keychain);
        CFRelease(keychain);
    }
    if (keychain_flags != NULL) free(keychain_flags);
    secure_zero(keychain_password, keychain_password_length);
    free(keychain_password);
    return result;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "--probe") == 0) {
        return run_code_signing_probe(argc, argv);
    }
    if (argc > 1 && strcmp(argv[1], "--archive") == 0) {
        return run_signed_archive(argc, argv);
    }
    if (argc > 1 && strcmp(argv[1], "--device-build") == 0) {
        return run_device_build(argc, argv);
    }
    /* --add imports a second identity into the keychain the first import created. */
    int add_mode = 0;
    if (argc > 1 && strcmp(argv[1], "--add") == 0) {
        add_mode = 1;
        argv++;
        argc--;
    }
    if (argc != 5) {
        fprintf(stderr, "invalid_arguments\n");
        return 1;
    }

    uint32_t keychain_password_length = 0;
    uint32_t certificate_password_length = 0;
    unsigned char *keychain_password = read_secret(&keychain_password_length);
    unsigned char *certificate_password = read_secret(&certificate_password_length);
    if (keychain_password == NULL || certificate_password == NULL) {
        fprintf(stderr, "invalid_secret_payload\n");
        if (keychain_password != NULL) {
            secure_zero(keychain_password, keychain_password_length);
            free(keychain_password);
        }
        if (certificate_password != NULL) {
            secure_zero(certificate_password, certificate_password_length);
            free(certificate_password);
        }
        return 1;
    }

    int result = 1;
    SecKeychainRef keychain = NULL;
    SecTrustedApplicationRef codesign = NULL;
    SecTrustedApplicationRef xcodebuild = NULL;
    SecAccessRef access = NULL;
    CFArrayRef search_list = NULL;
    CFMutableArrayRef updated_search_list = NULL;
    CFMutableArrayRef trusted_applications = NULL;
    CFStringRef certificate_passphrase = NULL;
    CFDataRef certificate_data = NULL;
    CFArrayRef key_attributes = NULL;
    CFArrayRef imported_items = NULL;
    unsigned char *certificate_bytes = NULL;
    size_t certificate_length = 0;

    OSStatus status;
    if (add_mode) {
        int unlock = open_and_unlock_keychain(
            argv[1], &keychain, keychain_password, keychain_password_length
        );
        if (unlock != 0) {
            result = unlock;
            goto cleanup;
        }
    } else {
        status = SecKeychainCreate(
            argv[1], keychain_password_length, keychain_password, false, NULL, &keychain
        );
        if (status != errSecSuccess) {
            result = security_failure("create_keychain", status);
            goto cleanup;
        }

        SecKeychainSettings settings = {
            SEC_KEYCHAIN_SETTINGS_VERS1,
            true,
            true,
            21600,
        };
        status = SecKeychainSetSettings(keychain, &settings);
        if (status != errSecSuccess) {
            result = security_failure("configure_keychain", status);
            goto cleanup;
        }

        status = SecKeychainCopySearchList(&search_list);
        if (status != errSecSuccess || search_list == NULL) {
            result = security_failure("read_keychain_search_list", status);
            goto cleanup;
        }
        updated_search_list = CFArrayCreateMutableCopy(
            NULL, CFArrayGetCount(search_list) + 1, search_list
        );
        if (updated_search_list == NULL) goto cleanup;
        if (!CFArrayContainsValue(
                updated_search_list,
                CFRangeMake(0, CFArrayGetCount(updated_search_list)),
                keychain
            )) {
            CFArrayAppendValue(updated_search_list, keychain);
        }
        status = SecKeychainSetSearchList(updated_search_list);
        if (status != errSecSuccess) {
            result = security_failure("update_keychain_search_list", status);
            goto cleanup;
        }
    }

    status = SecTrustedApplicationCreateFromPath("/usr/bin/codesign", &codesign);
    if (status != errSecSuccess) {
        result = security_failure("trust_codesign", status);
        goto cleanup;
    }
    status = SecTrustedApplicationCreateFromPath(argv[4], &xcodebuild);
    if (status != errSecSuccess) {
        result = security_failure("trust_xcodebuild", status);
        goto cleanup;
    }

    trusted_applications = CFArrayCreateMutable(NULL, 2, &kCFTypeArrayCallBacks);
    if (trusted_applications == NULL) goto cleanup;
    CFArrayAppendValue(trusted_applications, codesign);
    CFArrayAppendValue(trusted_applications, xcodebuild);
    status = SecAccessCreate(
        CFSTR("buildbridge signing identity"), trusted_applications, &access
    );
    if (status != errSecSuccess) {
        result = security_failure("create_access", status);
        goto cleanup;
    }

    certificate_bytes = read_file(argv[2], &certificate_length);
    if (certificate_bytes == NULL) {
        fprintf(stderr, "read_certificate\n");
        goto cleanup;
    }
    certificate_data = CFDataCreate(NULL, certificate_bytes, (CFIndex)certificate_length);
    certificate_passphrase = CFStringCreateWithBytes(
        NULL,
        certificate_password,
        certificate_password_length,
        kCFStringEncodingUTF8,
        false
    );
    const void *attribute_values[] = {kSecAttrIsPermanent, kSecAttrIsSensitive};
    key_attributes = CFArrayCreate(
        NULL, attribute_values, 2, &kCFTypeArrayCallBacks
    );
    if (certificate_data == NULL || certificate_passphrase == NULL || key_attributes == NULL) {
        fprintf(stderr, "prepare_certificate\n");
        goto cleanup;
    }

    SecExternalFormat format = kSecFormatPKCS12;
    SecExternalItemType item_type = kSecItemTypeAggregate;
    SecItemImportExportKeyParameters parameters = {0};
    parameters.version = SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION;
    parameters.passphrase = certificate_passphrase;
    parameters.accessRef = access;
    parameters.keyAttributes = key_attributes;
    status = SecItemImport(
        certificate_data,
        CFSTR(".p12"),
        &format,
        &item_type,
        0,
        &parameters,
        keychain,
        &imported_items
    );
    if (status != errSecSuccess) {
        result = security_failure("import_certificate", status);
        goto cleanup;
    }

    SecIdentityRef identity = NULL;
    CFIndex identity_count = 0;
    for (CFIndex index = 0; index < CFArrayGetCount(imported_items); index++) {
        CFTypeRef item = CFArrayGetValueAtIndex(imported_items, index);
        if (CFGetTypeID(item) == SecIdentityGetTypeID()) {
            identity_count++;
            if (identity == NULL) identity = (SecIdentityRef)item;
        }
    }
    if (identity_count != 1 || identity == NULL) {
        fprintf(stderr, "identity_count:%ld\n", (long)identity_count);
        goto cleanup;
    }

    status = authorize_identity_for_apple_signing(
        identity, keychain_password, keychain_password_length
    );
    if (status == errSecItemNotFound) {
        fprintf(stderr, "partition_acl_missing\n");
        goto cleanup;
    }
    if (status != errSecSuccess) {
        result = security_failure("authorize_private_key", status);
        goto cleanup;
    }

    SecCertificateRef certificate = NULL;
    status = SecIdentityCopyCertificate(identity, &certificate);
    if (status != errSecSuccess || certificate == NULL) {
        result = security_failure("read_identity_certificate", status);
        goto cleanup;
    }
    if (!write_certificate(argv[3], certificate)) {
        fprintf(stderr, "write_identity_certificate\n");
        CFRelease(certificate);
        goto cleanup;
    }
    CFRelease(certificate);

    printf("ok\n");
    result = 0;

cleanup:
    secure_zero(keychain_password, keychain_password_length);
    secure_zero(certificate_password, certificate_password_length);
    if (certificate_bytes != NULL) secure_zero(certificate_bytes, certificate_length);
    free(keychain_password);
    free(certificate_password);
    free(certificate_bytes);
    if (imported_items != NULL) CFRelease(imported_items);
    if (key_attributes != NULL) CFRelease(key_attributes);
    if (certificate_data != NULL) CFRelease(certificate_data);
    if (certificate_passphrase != NULL) CFRelease(certificate_passphrase);
    if (access != NULL) CFRelease(access);
    if (updated_search_list != NULL) CFRelease(updated_search_list);
    if (search_list != NULL) CFRelease(search_list);
    if (trusted_applications != NULL) CFRelease(trusted_applications);
    if (xcodebuild != NULL) CFRelease(xcodebuild);
    if (codesign != NULL) CFRelease(codesign);
    if (keychain != NULL) CFRelease(keychain);
    return result;
}
