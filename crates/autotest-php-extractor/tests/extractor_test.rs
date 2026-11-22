use autotest_php_extractor::PhpExtractor;

#[test]
fn test_php_extraction_invoice_service() {
    let php_code = r#"<?php
declare(strict_types=1);

namespace App\Billing;

final class InvoiceService
{
    public function __construct(
        private readonly TaxRepositoryInterface $taxRepo,
        private readonly ClockInterface $clock
    ) {}

    public function calculateTotal(float $amount, string $countryCode): Money
    {
        if ($amount < 0) {
            throw new \InvalidArgumentException("Negative amount");
        }
        if (strlen($countryCode) !== 2) {
            throw new \InvalidArgumentException("Invalid country code");
        }

        $rate = $this->taxRepo->getRateForCountry($countryCode);
        $time = $this->clock->now();

        return Money::fromCents((int)($amount * (1 + $rate) * 100), "EUR");
    }
}
"#;

    let doc = PhpExtractor::extract_source("src/Billing/InvoiceService.php", php_code).expect("extract");
    assert_eq!(doc.source.file, "src/Billing/InvoiceService.php");
    assert!(doc.source.content_hash.starts_with("sha256:"));
    assert_eq!(doc.targets.len(), 1);

    let target = &doc.targets[0];
    assert_eq!(target.id, "App\\Billing\\InvoiceService::calculateTotal");
    assert_eq!(target.signature.constructor_args.len(), 2);
    assert_eq!(target.signature.constructor_args[0].name, "taxRepo");
    assert_eq!(target.signature.constructor_args[1].name, "clock");

    assert_eq!(target.signature.parameters.len(), 2);
    assert_eq!(target.signature.parameters[0].name, "amount");
    assert!(target.signature.parameters[0].constraints.contains(&">= 0.0".to_string()));

    assert_eq!(target.signature.parameters[1].name, "countryCode");
    assert!(target.signature.parameters[1].constraints.contains(&"length == 2".to_string()));

    assert_eq!(target.signature.return_type.as_deref(), Some("Money"));
    assert!(target.detected_side_effects.iter().any(|s| s.target == "taxRepo.getRateForCountry"));
}

#[test]
fn test_php_extraction_with_default_parameters() {
    let php_code = r#"<?php
namespace App\Config;

class ServerConfig
{
    public function __construct(
        public string $host = "127.0.0.1",
        public int $port = 8080,
        public bool $ssl = false
    ) {}

    public function start(int $timeout = 30, string $env = 'production'): bool
    {
        return true;
    }
}
"#;
    let doc = PhpExtractor::extract_source("src/Config/ServerConfig.php", php_code).expect("extract");
    assert_eq!(doc.targets.len(), 1);
    let target = &doc.targets[0];
    assert_eq!(target.id, "App\\Config\\ServerConfig::start");

    let ctor = &target.signature.constructor_args;
    assert_eq!(ctor.len(), 3);
    assert_eq!(ctor[0].name, "host");
    assert_eq!(ctor[0].param_type, "string");
    assert_eq!(ctor[0].default_value, Some(serde_json::json!("127.0.0.1")));

    assert_eq!(ctor[1].name, "port");
    assert_eq!(ctor[1].param_type, "int");
    assert_eq!(ctor[1].default_value, Some(serde_json::json!(8080)));

    let params = &target.signature.parameters;
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "timeout");
    assert_eq!(params[0].param_type, "int");
    assert_eq!(params[0].default_value, Some(serde_json::json!(30)));

    assert_eq!(params[1].name, "env");
    assert_eq!(params[1].param_type, "string");
    assert_eq!(params[1].default_value, Some(serde_json::json!("production")));
}

#[test]
fn test_php_multi_class_no_leaks() {
    let php_code = r#"<?php
namespace App\Multi;

class ServiceA {
    public function __construct(private RepoA $repoA) {}
    public function actionA(): void {
        $this->repoA->save();
    }
}

class ServiceB {
    public function __construct(private RepoB $repoB) {}
    public function actionB(): void {
        $this->repoB->delete();
    }
}
"#;
    let doc = PhpExtractor::extract_source("src/Multi.php", php_code).expect("extract");
    assert_eq!(doc.targets.len(), 2);

    let target_a = doc.targets.iter().find(|t| t.id == "App\\Multi\\ServiceA::actionA").unwrap();
    assert_eq!(target_a.signature.constructor_args.len(), 1);
    assert_eq!(target_a.signature.constructor_args[0].name, "repoA");
    assert_eq!(target_a.detected_side_effects.len(), 1);
    assert_eq!(target_a.detected_side_effects[0].target, "repoA.save");

    let target_b = doc.targets.iter().find(|t| t.id == "App\\Multi\\ServiceB::actionB").unwrap();
    assert_eq!(target_b.signature.constructor_args.len(), 1);
    assert_eq!(target_b.signature.constructor_args[0].name, "repoB");
    assert_eq!(target_b.detected_side_effects.len(), 1);
    assert_eq!(target_b.detected_side_effects[0].target, "repoB.delete");
}
