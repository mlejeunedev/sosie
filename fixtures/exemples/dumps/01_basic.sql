-- MySQL dump 10.13  Distrib 8.0.36, for Linux (x86_64)
--
-- Host: replica-01    Database: shop
-- ------------------------------------------------------
-- Server version	8.0.36

/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!40101 SET NAMES utf8mb4 */;
/*!40014 SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0 */;
/*!40101 SET @OLD_SQL_MODE=@@SQL_MODE, SQL_MODE='NO_AUTO_VALUE_ON_ZERO' */;

--
-- Table structure for table `user`
--

DROP TABLE IF EXISTS `user`;
CREATE TABLE `user` (
  `id` int NOT NULL AUTO_INCREMENT,
  `email` varchar(180) COLLATE utf8mb4_unicode_ci NOT NULL,
  `first_name` varchar(100) COLLATE utf8mb4_unicode_ci NOT NULL,
  `last_name` varchar(100) COLLATE utf8mb4_unicode_ci NOT NULL,
  `phone` varchar(20) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `birth_date` date DEFAULT NULL,
  `password` varchar(255) COLLATE utf8mb4_unicode_ci NOT NULL,
  `api_token` varchar(64) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `nickname` varchar(50) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `roles` json NOT NULL,
  `created_at` datetime NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `UNIQ_8D93D649E7927C74` (`email`)
) ENGINE=InnoDB AUTO_INCREMENT=6 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

--
-- Dumping data for table `user`
--

LOCK TABLES `user` WRITE;
/*!40000 ALTER TABLE `user` DISABLE KEYS */;
INSERT INTO `user` VALUES (1,'jean.dupont@gmail.com','Jean','Dupont','0612345678','1985-03-14','$2y$13$abcdefghijklmnopqrstuuABCDEFGHIJKLMNOPQRSTUVWXYZ0123456','tok_a1b2c3d4e5f6','jeanjean','[\"ROLE_USER\"]','2023-01-10 09:12:44'),(2,'marie.o\'connor@example.com','Marie','O\'Connor','+33 7 98 76 54 32',NULL,'$2y$13$zyxwvutsrqponmlkjihgfeZYXWVUTSRQPONMLKJIHGFEDCBA987654','tok_ffffffffffff',NULL,'[\"ROLE_USER\",\"ROLE_ADMIN\"]','2023-02-01 18:00:00'),(3,'admin@shop.internal','Admin','System',NULL,NULL,'$2y$13$1111111111111111111111111111111111111111111111111111','tok_000000000000','root','[\"ROLE_SUPER_ADMIN\"]','2022-12-31 23:59:59'),(4,'lucas.martin@orange.fr','Lucas','Martin','0700000001','2001-11-30','$2y$13$2222222222222222222222222222222222222222222222222222',NULL,'lulu \\ le \"grand\"','[\"ROLE_USER\"]','2024-06-15 12:00:00'),(5,'jean.dupont@gmail.com','Jean','Dupont','0612345678','1985-03-14','$2y$13$3333333333333333333333333333333333333333333333333333',NULL,NULL,'[\"ROLE_USER\"]','2024-07-01 08:30:00');
/*!40000 ALTER TABLE `user` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `address`
--

DROP TABLE IF EXISTS `address`;
CREATE TABLE `address` (
  `id` int NOT NULL AUTO_INCREMENT,
  `user_id` int NOT NULL,
  `line1` varchar(255) COLLATE utf8mb4_unicode_ci NOT NULL,
  `line2` varchar(255) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `city` varchar(100) COLLATE utf8mb4_unicode_ci NOT NULL,
  `postcode` varchar(10) COLLATE utf8mb4_unicode_ci NOT NULL,
  `country` varchar(2) COLLATE utf8mb4_unicode_ci NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_D4E6F81A76ED395` (`user_id`),
  CONSTRAINT `FK_D4E6F81A76ED395` FOREIGN KEY (`user_id`) REFERENCES `user` (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=4 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

LOCK TABLES `address` WRITE;
/*!40000 ALTER TABLE `address` DISABLE KEYS */;
INSERT INTO `address` VALUES (1,1,'12 rue de la Paix','Bldg. B, 3rd floor','Paris','75002','FR'),(2,2,'4 impasse des Lilas',NULL,'Lyon','69003','FR'),(3,4,'Flat 2, 10 Downing Street',NULL,'London','SW1A 2AA','GB');
/*!40000 ALTER TABLE `address` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `order`
--

DROP TABLE IF EXISTS `order`;
CREATE TABLE `order` (
  `id` int NOT NULL AUTO_INCREMENT,
  `user_id` int NOT NULL,
  `reference` varchar(20) COLLATE utf8mb4_unicode_ci NOT NULL,
  `customer_email` varchar(180) COLLATE utf8mb4_unicode_ci NOT NULL,
  `amount` decimal(10,2) NOT NULL,
  `currency` varchar(3) COLLATE utf8mb4_unicode_ci NOT NULL,
  `status` enum('pending','paid','shipped','cancelled') COLLATE utf8mb4_unicode_ci NOT NULL,
  `notes` text COLLATE utf8mb4_unicode_ci,
  `placed_at` datetime NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_F5299398A76ED395` (`user_id`),
  CONSTRAINT `FK_F5299398A76ED395` FOREIGN KEY (`user_id`) REFERENCES `user` (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=5 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

LOCK TABLES `order` WRITE;
/*!40000 ALTER TABLE `order` DISABLE KEYS */;
INSERT INTO `order` VALUES (1,1,'ORD-2024-0001','jean.dupont@gmail.com',149.90,'EUR','paid','Deliver before 6pm, call Mr. Dupont at 06 12 34 56 78','2024-03-01 10:15:00'),(2,1,'ORD-2024-0002','jean.dupont@gmail.com',19.99,'EUR','shipped',NULL,'2024-03-05 16:40:12'),(3,2,'ORD-2024-0003','marie.o\'connor@example.com',1200.00,'EUR','pending','','2024-04-20 09:00:00'),(4,4,'ORD-2024-0004','lucas.martin@orange.fr',5.00,'GBP','cancelled','Customer says: \"I no longer want it\"\nRefunded.','2024-06-16 14:22:33');
/*!40000 ALTER TABLE `order` ENABLE KEYS */;
UNLOCK TABLES;

--
-- Table structure for table `bank_account`
--

DROP TABLE IF EXISTS `bank_account`;
CREATE TABLE `bank_account` (
  `id` int NOT NULL AUTO_INCREMENT,
  `user_id` int NOT NULL,
  `iban` varchar(34) COLLATE utf8mb4_unicode_ci NOT NULL,
  `bic` varchar(11) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `holder_name` varchar(200) COLLATE utf8mb4_unicode_ci NOT NULL,
  `is_default` tinyint(1) NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  CONSTRAINT `FK_53A23E0AA76ED395` FOREIGN KEY (`user_id`) REFERENCES `user` (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

LOCK TABLES `bank_account` WRITE;
INSERT INTO `bank_account` VALUES (1,1,'FR7630006000011234567890189','AGRIFRPP','Jean Dupont',1),(2,4,'GB29NWBK60161331926819','NWBKGB2L','L. Martin',1);
UNLOCK TABLES;

--
-- Table structure for table `product`  (reference data: no PII)
--

DROP TABLE IF EXISTS `product`;
CREATE TABLE `product` (
  `id` int NOT NULL AUTO_INCREMENT,
  `sku` varchar(32) COLLATE utf8mb4_unicode_ci NOT NULL,
  `name` varchar(255) COLLATE utf8mb4_unicode_ci NOT NULL,
  `price` decimal(10,2) NOT NULL,
  `image` blob,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

LOCK TABLES `product` WRITE;
INSERT INTO `product` VALUES (1,'SKU-001','Mechanical keyboard',89.00,0x89504E470D0A1A0A),(2,'SKU-002','27\" monitor',299.00,NULL);
UNLOCK TABLES;

--
-- Table structure for table `audit_log`  (to be emptied)
--

DROP TABLE IF EXISTS `audit_log`;
CREATE TABLE `audit_log` (
  `id` bigint NOT NULL AUTO_INCREMENT,
  `user_id` int DEFAULT NULL,
  `action` varchar(50) COLLATE utf8mb4_unicode_ci NOT NULL,
  `ip_address` varchar(45) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `payload` json DEFAULT NULL,
  `logged_at` datetime NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB AUTO_INCREMENT=3 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

LOCK TABLES `audit_log` WRITE;
INSERT INTO `audit_log` VALUES (1,1,'login','82.64.12.201','{\"ua\": \"Mozilla/5.0\", \"email\": \"jean.dupont@gmail.com\"}','2024-03-01 10:14:58'),(2,2,'login','2a01:cb00:1234::1',NULL,'2024-04-20 08:59:10');
UNLOCK TABLES;

/*!40101 SET SQL_MODE=@OLD_SQL_MODE */;
/*!40014 SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS */;
/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;

-- Dump completed on 2026-09-14 02:00:03
