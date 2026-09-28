-- Synthetic dump: mysqldump parser edge cases.
-- Main test: parse → serialize WITHOUT transformation == this file, byte for byte.
-- Secondary test: with 02_parser_edge_cases.yaml, only the `email` column changes.

/*!40101 SET NAMES utf8mb4 */;

-- 1. Reserved table name, columns with reserved names, column comment containing a comma and a parenthesis
DROP TABLE IF EXISTS `order`;
CREATE TABLE `order` (
  `id` int NOT NULL,
  `key` varchar(10) NOT NULL COMMENT 'key (unique), careful',
  `email` varchar(180) DEFAULT NULL,
  `desc` text,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- 2. Escapes: escaped apostrophe, backslash, double quote, newline, tab, NUL, empty string, NULL
INSERT INTO `order` VALUES (1,'a\'b','x@y.fr','line1\nline2\ttab\\backslash \"quoted\" \0nul'),(2,'','',''),(3,'c',NULL,NULL);

-- 3. INSERT over several physical lines (mysqldump --skip-extended-insert) + INSERT with a column list
INSERT INTO `order` VALUES (4,'d','d@d.fr','ok');
INSERT INTO `order` (`id`, `key`, `email`, `desc`) VALUES (5,'e','e@e.fr','with columns');

-- 4. Non-string values: hex, _binary, negative numbers, floats, dates, booleans
DROP TABLE IF EXISTS `misc`;
CREATE TABLE `misc` (
  `id` int NOT NULL,
  `blob_col` blob,
  `bin_col` varbinary(16) DEFAULT NULL,
  `f` double DEFAULT NULL,
  `d` decimal(12,4) DEFAULT NULL,
  `dt` datetime(6) DEFAULT NULL,
  `b` bit(1) DEFAULT NULL,
  `email` varchar(180) DEFAULT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

INSERT INTO `misc` VALUES (1,0xDEADBEEF,_binary '\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0',-1.5e-10,-12345.6789,'2024-02-29 23:59:59.123456',_binary '\0','m@m.fr'),(2,NULL,NULL,NULL,NULL,NULL,NULL,NULL);

-- 5. A value that looks like SQL inside a string (must NOT be interpreted)
INSERT INTO `misc` VALUES (3,NULL,NULL,NULL,NULL,NULL,NULL,'INSERT INTO `user` VALUES (1,''x''); -- not a real query');

-- 6. Generated column, column with a DEFAULT expression, CHECK constraint, fulltext index
DROP TABLE IF EXISTS `gen`;
CREATE TABLE `gen` (
  `id` int NOT NULL,
  `email` varchar(180) NOT NULL,
  `email_domain` varchar(180) GENERATED ALWAYS AS (substring_index(`email`,_utf8mb4'@',-(1))) VIRTUAL,
  `created` datetime DEFAULT CURRENT_TIMESTAMP,
  `qty` int DEFAULT '0',
  PRIMARY KEY (`id`),
  FULLTEXT KEY `ft_email` (`email`),
  CONSTRAINT `chk_qty` CHECK ((`qty` >= 0))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- mysqldump omits generated columns from INSERTs: there are only 4 values, not 5
INSERT INTO `gen` (`id`, `email`, `created`, `qty`) VALUES (1,'g@g.fr','2024-01-01 00:00:00',3);

-- 7. View and trigger: passed through as is (Raw), with a warning in the report
DROP VIEW IF EXISTS `v_emails`;
/*!50001 CREATE ALGORITHM=UNDEFINED */
/*!50013 DEFINER=`app`@`%` SQL SECURITY DEFINER */
/*!50001 VIEW `v_emails` AS select `order`.`email` AS `email` from `order` */;

DELIMITER ;;
/*!50003 CREATE*/ /*!50017 DEFINER=`app`@`%`*/ /*!50003 TRIGGER `trg_order` BEFORE INSERT ON `order` FOR EACH ROW SET NEW.`key` = LOWER(NEW.`key`) */;;
DELIMITER ;

-- 8. Table without any row (no INSERT)
DROP TABLE IF EXISTS `empty_table`;
CREATE TABLE `empty_table` (
  `id` int NOT NULL,
  `email` varchar(180) DEFAULT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- 9. Unicode, emoji, 4-byte characters, typographic apostrophe (not escaped since ≠ ')
INSERT INTO `empty_table` VALUES (1,'émoji-😀-中文-Œuvre-l’apostrophe@x.fr');

-- Dump completed
