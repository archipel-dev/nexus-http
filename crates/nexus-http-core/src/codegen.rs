//! Code generation: turns a prepared (rendered) request into a ready-to-run
//! snippet for common clients.

use crate::exec::PreparedRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Curl,
    PythonRequests,
    JavaScriptFetch,
    Go,
    RustReqwest,
    JavaHttpClient,
    KotlinOkHttp,
}

impl Target {
    pub const ALL: [Target; 7] = [
        Target::Curl,
        Target::PythonRequests,
        Target::JavaScriptFetch,
        Target::Go,
        Target::RustReqwest,
        Target::JavaHttpClient,
        Target::KotlinOkHttp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Target::Curl => "Shell — curl",
            Target::PythonRequests => "Python — requests",
            Target::JavaScriptFetch => "JavaScript — fetch",
            Target::Go => "Go — net/http",
            Target::RustReqwest => "Rust — reqwest",
            Target::JavaHttpClient => "Java — HttpClient",
            Target::KotlinOkHttp => "Kotlin — OkHttp",
        }
    }

    /// Language id for syntax highlighting.
    pub fn language(self) -> &'static str {
        match self {
            Target::Curl => "shell",
            Target::PythonRequests => "python",
            Target::JavaScriptFetch => "javascript",
            Target::Go => "go",
            Target::RustReqwest => "rust",
            Target::JavaHttpClient => "java",
            Target::KotlinOkHttp => "kotlin",
        }
    }
}

/// Generates a snippet for `target`.
pub fn generate(request: &PreparedRequest, target: Target) -> String {
    let body = match std::str::from_utf8(&request.body) {
        Ok(text) => text.to_owned(),
        Err(_) => String::new(),
    };
    let binary = !request.body.is_empty() && std::str::from_utf8(&request.body).is_err();
    let headers: Vec<(&str, &str)> = request
        .headers
        .iter()
        .filter(|(n, _)| !n.eq_ignore_ascii_case("content-length"))
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let method = request.method.as_str();
    let url = request.url.as_str();
    let mut out = match target {
        Target::Curl => curl(method, url, &headers, &body),
        Target::PythonRequests => python(method, url, &headers, &body),
        Target::JavaScriptFetch => fetch(method, url, &headers, &body),
        Target::Go => go(method, url, &headers, &body),
        Target::RustReqwest => rust(method, url, &headers, &body),
        Target::JavaHttpClient => java(method, url, &headers, &body),
        Target::KotlinOkHttp => kotlin(method, url, &headers, &body),
    };
    if binary {
        let comment = match target {
            Target::Curl | Target::PythonRequests => "#",
            _ => "//",
        };
        out = format!(
            "{comment} The {}-byte binary body is not included; load it from its file.\n{out}",
            request.body.len()
        );
    }
    out
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A double-quoted literal valid in JSON, JavaScript, Python, Go and Java.
fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn rust_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kotlin_quote(s: &str) -> String {
    quote(s).replace('$', "\\$")
}

fn curl(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut lines = vec![format!(
        "curl --request {method} \\\n  --url {}",
        shell_quote(url)
    )];
    for (n, v) in headers {
        lines.push(format!("  --header {}", shell_quote(&format!("{n}: {v}"))));
    }
    if !body.is_empty() {
        lines.push(format!("  --data {}", shell_quote(body)));
    }
    lines.join(" \\\n") + "\n"
}

fn python(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = String::from("import requests\n\n");
    out.push_str(&format!("url = {}\n", quote(url)));
    if !body.is_empty() {
        out.push_str(&format!("payload = {}\n", quote(body)));
    }
    if !headers.is_empty() {
        out.push_str("headers = {\n");
        for (n, v) in headers {
            out.push_str(&format!("    {}: {},\n", quote(n), quote(v)));
        }
        out.push_str("}\n");
    }
    out.push_str(&format!(
        "\nresponse = requests.request({}, url",
        quote(method)
    ));
    if !body.is_empty() {
        out.push_str(", data=payload");
    }
    if !headers.is_empty() {
        out.push_str(", headers=headers");
    }
    out.push_str(")\n\nprint(response.status_code)\nprint(response.text)\n");
    out
}

fn fetch(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!(
        "const response = await fetch({}, {{\n  method: {},\n",
        quote(url),
        quote(method)
    );
    if !headers.is_empty() {
        out.push_str("  headers: {\n");
        for (n, v) in headers {
            out.push_str(&format!("    {}: {},\n", quote(n), quote(v)));
        }
        out.push_str("  },\n");
    }
    if !body.is_empty() {
        out.push_str(&format!("  body: {},\n", quote(body)));
    }
    out.push_str("});\n\nconsole.log(response.status);\nconsole.log(await response.text());\n");
    out
}

fn go(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = String::from("package main\n\nimport (\n\t\"fmt\"\n\t\"io\"\n\t\"net/http\"\n");
    if !body.is_empty() {
        out.push_str("\t\"strings\"\n");
    }
    out.push_str(")\n\nfunc main() {\n");
    let body_arg = if body.is_empty() {
        "nil".to_owned()
    } else {
        out.push_str(&format!(
            "\tpayload := strings.NewReader({})\n",
            quote(body)
        ));
        "payload".to_owned()
    };
    out.push_str(&format!(
        "\treq, err := http.NewRequest({}, {}, {body_arg})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n",
        quote(method),
        quote(url)
    ));
    for (n, v) in headers {
        out.push_str(&format!("\treq.Header.Add({}, {})\n", quote(n), quote(v)));
    }
    out.push_str(
        "\n\tres, err := http.DefaultClient.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer res.Body.Close()\n\tbody, _ := io.ReadAll(res.Body)\n\n\tfmt.Println(res.Status)\n\tfmt.Println(string(body))\n}\n",
    );
    out
}

fn rust(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = String::from(
        "// Cargo.toml: reqwest = { version = \"0.12\", features = [\"blocking\"] }\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let client = reqwest::blocking::Client::new();\n",
    );
    out.push_str(&format!(
        "    let response = client\n        .request(reqwest::Method::from_bytes(b{})?, {})\n",
        rust_quote(method),
        rust_quote(url)
    ));
    for (n, v) in headers {
        out.push_str(&format!(
            "        .header({}, {})\n",
            rust_quote(n),
            rust_quote(v)
        ));
    }
    if !body.is_empty() {
        out.push_str(&format!("        .body({})\n", rust_quote(body)));
    }
    out.push_str("        .send()?;\n\n    println!(\"{}\", response.status());\n    println!(\"{}\", response.text()?);\n    Ok(())\n}\n");
    out
}

fn java(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = String::from(
        "import java.net.URI;\nimport java.net.http.HttpClient;\nimport java.net.http.HttpRequest;\nimport java.net.http.HttpResponse;\n\npublic class Main {\n    public static void main(String[] args) throws Exception {\n        HttpRequest request = HttpRequest.newBuilder()\n",
    );
    out.push_str(&format!("            .uri(URI.create({}))\n", quote(url)));
    for (n, v) in headers {
        out.push_str(&format!(
            "            .header({}, {})\n",
            quote(n),
            quote(v)
        ));
    }
    let publisher = if body.is_empty() {
        "HttpRequest.BodyPublishers.noBody()".to_owned()
    } else {
        format!("HttpRequest.BodyPublishers.ofString({})", quote(body))
    };
    out.push_str(&format!(
        "            .method({}, {publisher})\n            .build();\n",
        quote(method)
    ));
    out.push_str(
        "        HttpResponse<String> response = HttpClient.newHttpClient()\n            .send(request, HttpResponse.BodyHandlers.ofString());\n        System.out.println(response.statusCode());\n        System.out.println(response.body());\n    }\n}\n",
    );
    out
}

fn kotlin(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = String::from("import okhttp3.OkHttpClient\nimport okhttp3.Request\n");
    let content_type = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| *v);
    if !body.is_empty() {
        out.push_str("import okhttp3.MediaType.Companion.toMediaTypeOrNull\nimport okhttp3.RequestBody.Companion.toRequestBody\n");
    }
    out.push_str("\nfun main() {\n    val client = OkHttpClient()\n");
    let body_arg = if body.is_empty() {
        if matches!(method, "POST" | "PUT" | "PATCH") {
            out.push_str("    val body = ByteArray(0).toRequestBody(null)\n");
            "body"
        } else {
            "null"
        }
    } else {
        out.push_str(&format!(
            "    val body = {}.toRequestBody({}.toMediaTypeOrNull())\n",
            kotlin_quote(body),
            kotlin_quote(content_type.unwrap_or("text/plain"))
        ));
        "body"
    };
    if body_arg == "body" && body.is_empty() {
        out = out.replacen(
            "import okhttp3.Request\n",
            "import okhttp3.Request\nimport okhttp3.RequestBody.Companion.toRequestBody\n",
            1,
        );
    }
    out.push_str(&format!(
        "    val request = Request.Builder()\n        .url({})\n        .method({}, {body_arg})\n",
        kotlin_quote(url),
        kotlin_quote(method)
    ));
    for (n, v) in headers {
        out.push_str(&format!(
            "        .addHeader({}, {})\n",
            kotlin_quote(n),
            kotlin_quote(v)
        ));
    }
    out.push_str("        .build()\n\n    client.newCall(request).execute().use { response ->\n        println(response.code)\n        println(response.body?.string())\n    }\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{DeferredAuth, Transport};

    fn request(body: &str) -> PreparedRequest {
        PreparedRequest {
            request_id: None,
            method: "POST".into(),
            url: url::Url::parse("https://api.test/items?q=1").unwrap(),
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("X-Price".into(), "$5".into()),
            ],
            body: body.as_bytes().to_vec(),
            auth: DeferredAuth::None,
            transport: Transport {
                follow_redirects: true,
                max_redirects: 10,
                verify_tls: true,
                timeout_ms: 0,
                use_cookies: true,
                max_response_bytes: 1,
            },
        }
    }

    #[test]
    fn curl_quotes_for_shell() {
        let s = generate(&request("{\"a\":\"it's\"}"), Target::Curl);
        assert!(
            s.starts_with("curl --request POST \\\n  --url 'https://api.test/items?q=1'"),
            "{s}"
        );
        assert!(s.contains("--header 'X-Price: $5'"));
        assert!(s.contains("--data '{\"a\":\"it'\\''s\"}'"));
        // The generated command parses back to the same request.
        let back = crate::import::curl::parse(&s).unwrap();
        assert_eq!(back.method.as_str(), "POST");
        assert_eq!(
            back.body,
            crate::model::Body::Json {
                text: "{\"a\":\"it's\"}".into()
            }
        );
    }

    #[test]
    fn every_target_includes_method_url_headers_and_body() {
        let req = request("line1\nline2");
        for target in Target::ALL {
            let s = generate(&req, target);
            assert!(s.contains("POST"), "{target:?}: {s}");
            assert!(s.contains("api.test/items"), "{target:?}");
            assert!(s.contains("X-Price"), "{target:?}");
            assert!(s.contains("line1"), "{target:?}");
            assert!(!target.label().is_empty() && !target.language().is_empty());
        }
    }

    #[test]
    fn escapes_per_language() {
        let req = request("\"q\"\n");
        assert!(generate(&req, Target::RustReqwest).contains(".body(\"\\\"q\\\"\\n\")"));
        assert!(generate(&req, Target::KotlinOkHttp).contains("\"\\$5\""));
        assert!(generate(&req, Target::PythonRequests).contains("payload = \"\\\"q\\\"\\n\""));
        let mut empty = request("");
        empty.method = "GET".into();
        let go = generate(&empty, Target::Go);
        assert!(go.contains("http.NewRequest(\"GET\", \"https://api.test/items?q=1\", nil)"));
        assert!(!go.contains("strings"));
        let mut binary = request("");
        binary.body = vec![0xff, 0x00];
        assert!(
            generate(&binary, Target::JavaScriptFetch).starts_with("// The 2-byte binary body")
        );
    }
}
