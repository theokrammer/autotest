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
