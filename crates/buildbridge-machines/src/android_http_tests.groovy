// Run with Groovy from a local Gradle distribution; see the ignored Rust integration test.
import groovy.json.JsonSlurper
import javax.xml.parsers.DocumentBuilderFactory

class ManifestInput {
    File srcFile
    void srcFile(File value) { srcFile = value }
}
class DirectoryInput {
    Set<File> srcDirs
    void setSrcDirs(Collection<File> values) { this.@srcDirs = values as Set }
}
class SourceInput {
    ManifestInput manifest
    DirectoryInput assets
    DirectoryInput res
}
class NamedInputs {
    Map<String, SourceInput> inputs
    SourceInput getByName(String name) { inputs[name] }
}

def source = new File(args[0])
def directory = new File(args[1])
def write = { String path, String value ->
    def file = new File(directory, path)
    file.parentFile.mkdirs()
    file.setText(value, 'UTF-8')
    file
}
def makeInputs = {
    new NamedInputs(inputs: ['main', 'debug', 'release'].collectEntries { name ->
        [(name): new SourceInput(
            manifest: new ManifestInput(srcFile: new File(directory, "src/$name/AndroidManifest.xml")),
            assets: new DirectoryInput(srcDirs: [new File(directory, "src/$name/assets")] as Set),
            res: new DirectoryInput(srcDirs: [new File(directory, "src/$name/res")] as Set))]
    })
}
def invoke = { inputs ->
    def android = new Expando(sourceSets: inputs)
    def components = new Expando(finalizeDsl: { callback -> callback(android) })
    def project = new Expando(
        buildDir: new File(directory, 'build'),
        delete: { File file -> file.deleteDir() },
        pluginManager: new Expando(withPlugin: { name, callback ->
            assert name == 'com.android.application'
            callback()
        }),
        extensions: new Expando(getByName: { name ->
            assert name == 'androidComponents'
            components
        }))
    def gradle = new Expando(beforeProject: { callback -> callback(project) })
    new GroovyShell(new Binding([gradle: gradle])).evaluate(source)
}
def parseXml = { File file ->
    def factory = DocumentBuilderFactory.newInstance()
    factory.namespaceAware = true
    factory.newDocumentBuilder().parse(file)
}
def originalFiles = [:]
def snapshot = {
    new File(directory, 'src').eachFileRecurse { file ->
        if (file.isFile()) originalFiles[file] = file.getText('UTF-8')
    }
}
def assertOriginals = {
    originalFiles.each { file, contents -> assert file.getText('UTF-8') == contents }
}

write('src/main/AndroidManifest.xml', '''<manifest xmlns:a="http://schemas.android.com/apk/res/android">
  <application a:usesCleartextTraffic="false" a:networkSecurityConfig="@xml/network"/>
</manifest>''')
write('src/main/assets/capacitor.config.json', '''{"appId":"example.main","android":{"allowMixedContent":false,"webContentsDebuggingEnabled":false},"server":{"hostname":"localhost","androidScheme":"https"},"plugins":{"Keep":{"value":"main"}}}''')
write('src/debug/AndroidManifest.xml', '''<manifest xmlns:a="http://schemas.android.com/apk/res/android" xmlns:t="http://schemas.android.com/tools">
  <uses-permission a:name="example.permission.DEBUG"/>
  <application a:usesCleartextTraffic="false" a:label="Debug app" t:remove="a:usesCleartextTraffic,a:backupAgent" t:replace="a:label" t:strict="a:usesCleartextTraffic">
    <meta-data a:name="keep" a:value="yes"/>
  </application>
</manifest>''')
write('src/debug/assets/capacitor.config.json', '''{"appId":"example.debug","android":{"allowMixedContent":false,"keepDebugSetting":17},"server":{"androidScheme":"https"},"plugins":{"Keep":{"value":"debug"}}}''')
write('src/debug/assets/keep.txt', 'custom debug asset')
write('src/debug/res/values/strings.xml', '<resources><string name="keep">debug name</string></resources>')
write('src/main/res/xml/network.xml', '''<network-security-config>
  <base-config cleartextTrafficPermitted="false"><trust-anchors><certificates src="@raw/custom_ca"/></trust-anchors></base-config>
  <domain-config cleartextTrafficPermitted="false"><domain includeSubdomains="true">example.com</domain>
    <pin-set><pin digest="SHA-256">KEEP_PIN</pin></pin-set>
    <domain-config cleartextTrafficPermitted="false"><domain>nested.example.com</domain></domain-config>
  </domain-config>
  <debug-overrides><trust-anchors><certificates src="@raw/debug_ca"/></trust-anchors></debug-overrides>
</network-security-config>''')
write('src/main/res/xml-v28/network.xml', '<network-security-config><domain-config cleartextTrafficPermitted="false"><domain>main-version.example.com</domain></domain-config></network-security-config>')
write('src/debug/res/xml-v28/network.xml', '<network-security-config><domain-config><domain>debug-version.example.com</domain><trust-anchors><certificates src="system" overridePins="false"/></trust-anchors></domain-config></network-security-config>')
write('src/release/assets/capacitor.config.json', '{"android":{"allowMixedContent":false}}')
snapshot()

def inputs = makeInputs()
def originalMain = inputs.getByName('main').assets.srcDirs
def originalRelease = inputs.getByName('release').assets.srcDirs
invoke(inputs)
def debug = inputs.getByName('debug')
def assets = debug.assets.srcDirs.first()
def config = new JsonSlurper().parse(new File(assets, 'capacitor.config.json'))
assert config.appId == 'example.debug'
assert config.android.allowMixedContent
assert config.android.keepDebugSetting == 17
assert config.server.androidScheme == 'https'
assert config.plugins.Keep.value == 'debug'
assert new File(assets, 'keep.txt').text == 'custom debug asset'
assert inputs.getByName('main').assets.srcDirs == originalMain
assert inputs.getByName('release').assets.srcDirs == originalRelease
def manifest = parseXml(debug.manifest.srcFile)
def app = manifest.getElementsByTagName('application').item(0)
def a = 'http://schemas.android.com/apk/res/android'
def t = 'http://schemas.android.com/tools'
assert app.getAttributeNS(a, 'usesCleartextTraffic') == 'true'
assert app.getAttributeNS(a, 'label') == 'Debug app'
assert app.getAttributeNS(t, 'replace').contains('a:label')
assert app.getAttributeNS(t, 'remove') == 'a:backupAgent'
assert app.getAttributeNS(t, 'strict') == ''
assert app.getElementsByTagName('meta-data').item(0).getAttributeNS(a, 'value') == 'yes'
assert manifest.getElementsByTagName('uses-permission').length == 1
def resources = debug.res.srcDirs.first()
assert new File(resources, 'values/strings.xml').text.contains('debug name')
['xml', 'xml-v28'].each { qualifier ->
    def network = parseXml(new File(resources, "$qualifier/network.xml"))
    ['base-config', 'domain-config'].each { name ->
        def nodes = network.getElementsByTagName(name)
        assert nodes.length > 0
        for (int i = 0; i < nodes.length; i++) {
            assert nodes.item(i).getAttribute('cleartextTrafficPermitted') == 'true'
        }
    }
}
def network = new File(resources, 'xml/network.xml').text
assert network.contains('@raw/custom_ca') && network.contains('KEEP_PIN')
assert network.contains('@raw/debug_ca')
def qualified = new File(resources, 'xml-v28/network.xml').text
assert qualified.contains('debug-version.example.com') && !qualified.contains('main-version.example.com')
assert qualified.contains('overridePins="false"')
assertOriginals()

// A later off/release invocation reconstructs the original DSL inputs even with all
// generated files left behind (including after a process failure or cancellation).
def later = makeInputs()
assert !new JsonSlurper().parse(new File(later.getByName('debug').assets.srcDirs.first(), 'capacitor.config.json')).android.allowMixedContent
assert !new JsonSlurper().parse(new File(later.getByName('release').assets.srcDirs.first(), 'capacitor.config.json')).android.allowMixedContent

// Without project debug config/manifest, inherit all main Capacitor settings. Re-running
// also discards earlier generated assets/resources instead of accidentally reusing them.
new File(directory, 'src/debug/assets/capacitor.config.json').delete()
new File(directory, 'src/debug/AndroidManifest.xml').delete()
new File(directory, 'src/debug/assets/keep.txt').delete()
def inherited = makeInputs()
invoke(inherited)
def inheritedAssets = inherited.getByName('debug').assets.srcDirs.first()
def inheritedConfig = new JsonSlurper().parse(new File(inheritedAssets, 'capacitor.config.json'))
assert inheritedConfig.appId == 'example.main'
assert inheritedConfig.android.allowMixedContent
assert !inheritedConfig.android.webContentsDebuggingEnabled
assert inheritedConfig.plugins.Keep.value == 'main'
assert !new File(inheritedAssets, 'keep.txt').exists()
assert parseXml(inherited.getByName('debug').manifest.srcFile).getElementsByTagName('application').length == 1

// A policy supplied only by an external library must not silently keep HTTP blocked
// or be replaced with a default that drops its trust settings.
def mainManifest = new File(directory, 'src/main/AndroidManifest.xml')
def originalManifest = mainManifest.text
mainManifest.setText(originalManifest.replace('@xml/network', '@xml/library_policy'), 'UTF-8')
try {
    invoke(makeInputs())
    assert false: 'An external network policy needs an actionable error'
} catch (IllegalStateException expected) {
    assert expected.message.contains('network security XML')
}
assert mainManifest.text.contains('@xml/library_policy')
mainManifest.setText(originalManifest, 'UTF-8')

// A preparation failure cannot alter source settings either.
def mainConfig = new File(directory, 'src/main/assets/capacitor.config.json')
mainConfig.setText('{"android":17}', 'UTF-8')
try {
    invoke(makeInputs())
    assert false: 'Invalid config should fail'
} catch (IllegalStateException expected) {
    assert expected.message.contains('Android settings object')
}
assert mainConfig.text == '{"android":17}'
assert new File(directory, 'src/main/AndroidManifest.xml').text == originalFiles[new File(directory, 'src/main/AndroidManifest.xml')]
println 'Android HTTP overlay fixtures passed'
